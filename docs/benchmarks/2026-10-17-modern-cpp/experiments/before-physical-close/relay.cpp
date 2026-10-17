// WAR CBOR relay: sockets are thread-confined; routing metadata is shared.
#include "relay.hpp"
#include "App.h"
#include "cbor.hpp"
#include <algorithm>
#include <atomic>
#include <barrier>
#include <chrono>
#include <csignal>
#include <cstdlib>
#include <iostream>
#include <memory>
#include <mutex>
#include <pthread.h>
#include <sched.h>
#include <shared_mutex>
#include <sys/random.h>
#include <sys/socket.h>
#include <thread>
#include <unordered_map>
#include <vector>

namespace war {
constexpr uint64_t broadcast = UINT64_MAX;
constexpr View global = "re_globl";
struct Worker;
struct Peer;
using Ref = std::shared_ptr<Peer>;
struct SocketData {
  Ref peer;
};
using Socket = uWS::WebSocket<false, true, SocketData>;
struct Peer {
  Worker *owner;
  Socket *socket = nullptr; // accessed exclusively by owner loop
  uint64_t id = 0;
  std::string channel;
  bool joined = false, authenticated = false;
  unsigned flags = 0;        // protected by registry mutex
  size_t buffered_bytes = 0; // owner-thread accounting of uWS output storage
  std::atomic<bool> closing{false};
  // A remote producer reserves before publication. Release after owner submits.
  std::atomic<size_t> pending_bytes{0}, pending_messages{0};
  explicit Peer(Worker *worker) : owner(worker) {}
};
struct Channel {
  std::vector<Ref> members;
  std::unordered_map<std::string, std::string> variables;
  size_t variable_bytes = 0;
};
struct Registry {
  std::shared_mutex mutex;
  std::unordered_map<uint64_t, Ref> users;
  std::unordered_map<std::string, Channel> channels;
  std::array<std::vector<Ref>, 3> observers;
  size_t variables = 0;
};
enum class CloseReason : uint8_t { Automatic, Reserved, Global };
struct Delivery {
  Ref peer;
  std::shared_ptr<const std::string> body;
  uint16_t close_code = 0;
  CloseReason reason = CloseReason::Automatic;
};
static_assert(sizeof(Delivery) == 40);
struct Runtime {
  Config config;
  Registry registry;
  std::vector<std::unique_ptr<Worker>> workers;
  std::atomic<bool> stopping{false}, startup_failed{false};
  std::atomic<unsigned> quiesced{0};
  std::atomic<size_t> connections{0}, queued{0}, buffered{0}, retained_bytes{0};
  std::atomic<uint64_t> sent{0}, deliveries{0}, value_bytes{0}, disconnected{0},
      rejected{0}, protocol_errors{0}, slow{0}, borrowed{0}, copied{0};
  explicit Runtime(Config c) : config(std::move(c)) {
    registry.channels.try_emplace(std::string(global));
    registry.users.reserve(std::min(config.max_connections, size_t(110000)));
  }
  bool reserve(std::atomic<size_t> &counter, size_t bytes, size_t limit) {
    auto current = counter.load(std::memory_order_relaxed);
    do {
      if (current > limit || bytes > limit - current)
        return false;
    } while (!counter.compare_exchange_weak(current, current + bytes,
                                            std::memory_order_relaxed));
    return true;
  }
  std::string metrics();
};
struct Worker {
  Runtime &runtime;
  unsigned index;
  uWS::Loop *loop = nullptr;
  uWS::App *application = nullptr;
  us_timer_t *keepalive = nullptr;
  us_listen_socket_t *listener = nullptr, *admin_listener = nullptr;
  std::unordered_map<Peer *, Ref> live;
  std::mutex inbox_mutex;
  std::vector<Delivery> inbox, consuming;
  size_t consume_position = 0;
  bool scheduled = false;
  std::string scratch;
  Worker(Runtime &r, unsigned i) : runtime(r), index(i) {
    scratch.reserve(4096);
    inbox.reserve(256);
    consuming.reserve(256);
  }
  // end() can synchronously invoke close(). Always call it outside metadata
  // guards.
  void request_close(const Ref &peer, uint16_t code,
                     CloseReason reason = CloseReason::Automatic) {
    if (peer->closing.exchange(true, std::memory_order_relaxed))
      return;
    post({peer, {}, code, reason});
  }
  void post(Delivery delivery) {
    std::lock_guard guard(inbox_mutex);
    inbox.push_back(std::move(delivery));
    if (!scheduled) {
      scheduled = true;
      loop->defer([this] { drain(); });
    }
  }
  size_t buffered_amount(const Ref &p) {
    if (!p->socket)
      return 0;
    // Pinned uWS adapter: include automatic receive-callback cork storage.
    auto *data = static_cast<uWS::LoopData *>(
        us_loop_ext(reinterpret_cast<us_loop_t *>(loop)));
    return p->socket->getBufferedAmount() +
           (data->corkedSocket == p->socket ? data->corkOffset : 0);
  }
  void reconcile_buffer(const Ref &p) {
    size_t now = buffered_amount(p);
    if (now > p->buffered_bytes) {
      auto extra = now - p->buffered_bytes;
      runtime.buffered.fetch_add(extra, std::memory_order_relaxed);
      runtime.retained_bytes.fetch_add(extra, std::memory_order_relaxed);
      if (runtime.retained_bytes.load(std::memory_order_relaxed) >
          runtime.config.total_queue_bytes)
        request_close(p, 1013);
    } else {
      auto released = p->buffered_bytes - now;
      runtime.buffered.fetch_sub(released, std::memory_order_relaxed);
      runtime.retained_bytes.fetch_sub(released, std::memory_order_relaxed);
    }
    p->buffered_bytes = now;
  }
  void write(const Ref &p, View body) {
    if (!p->socket || p->closing.load(std::memory_order_relaxed))
      return;
    reconcile_buffer(p);
    auto &c = runtime.config;
    size_t before = p->buffered_bytes;
    if (body.size() + 10 > c.queue_bytes ||
        before > c.queue_bytes - body.size() - 10) {
      ++runtime.slow;
      request_close(p, 1013);
      return;
    }
    size_t credit = body.size() + 10;
    if (!runtime.reserve(runtime.retained_bytes, credit, c.total_queue_bytes)) {
      ++runtime.slow;
      request_close(p, 1013);
      return;
    }
    auto status = p->socket->send(body, uWS::BINARY);
    size_t after = buffered_amount(p);
    require(after <= before + credit);
    runtime.retained_bytes.fetch_sub(before + credit - after,
                                     std::memory_order_relaxed);
    if (after >= before)
      runtime.buffered.fetch_add(after - before, std::memory_order_relaxed);
    else
      runtime.buffered.fetch_sub(before - after, std::memory_order_relaxed);
    p->buffered_bytes = after;
    if (status == Socket::DROPPED) {
      ++runtime.slow;
      request_close(p, 1013);
    } else
      ++runtime.deliveries;
  }
  void submit(const Ref &p, View body,
              std::shared_ptr<const std::string> &retained) {
    if (p->closing.load(std::memory_order_relaxed))
      return;
    // Pending remote submissions precede local replies, including permission
    // ACKs.
    if (p->owner == this &&
        p->pending_messages.load(std::memory_order_acquire) == 0) {
      write(p, body);
      return;
    }
    auto &c = runtime.config;
    if (!runtime.reserve(p->pending_messages, 1, c.queue_messages)) {
      ++runtime.slow;
      p->owner->request_close(p, 1013);
      return;
    }
    if (!runtime.reserve(p->pending_bytes, body.size(), c.queue_bytes)) {
      --p->pending_messages;
      ++runtime.slow;
      p->owner->request_close(p, 1013);
      return;
    }
    if (!runtime.reserve(runtime.retained_bytes, body.size(),
                         c.total_queue_bytes)) {
      p->pending_bytes -= body.size();
      --p->pending_messages;
      ++runtime.slow;
      p->owner->request_close(p, 1013);
      return;
    }
    runtime.queued.fetch_add(body.size(), std::memory_order_relaxed);
    if (!retained) {
      retained = std::make_shared<const std::string>(body);
      runtime.copied += body.size();
    }
    p->owner->post({p, retained, 0});
  }
  void send(const Ref &p, View body) {
    std::shared_ptr<const std::string> retained;
    submit(p, body, retained);
  }
  void drain() {
    // Bound one callback by recipient visits so queued fanout cannot monopolize
    // a loop.
    for (unsigned n = 0; n < 256; ++n) {
      if (consume_position == consuming.size()) {
        consuming.clear();
        consume_position = 0;
        std::lock_guard guard(inbox_mutex);
        if (inbox.empty()) {
          scheduled = false;
          return;
        }
        consuming.swap(inbox);
      }
      Delivery d = std::move(consuming[consume_position++]);
      if (d.close_code) {
        if (d.peer->socket)
          d.peer->socket->end(d.close_code,
                              d.reason == CloseReason::Reserved
                                  ? "Reserved UserID"
                              : d.reason == CloseReason::Global
                                  ? "Global Broadcast Requires Authentication"
                              : d.close_code == 4001 ? "UserID Taken"
                              : d.close_code == 1008 ? "Authentication Required"
                              : d.close_code == 1003 ? "Type Unsupported"
                                                     : "Try Again Later");
      } else {
        write(d.peer, *d.body);
        d.peer->pending_bytes -= d.body->size();
        --d.peer->pending_messages;
        runtime.queued -= d.body->size();
        runtime.retained_bytes -= d.body->size();
      }
    }
    loop->defer([this] { drain(); });
  }
  static uint64_t random_id() {
    uint64_t id;
    size_t filled = 0;
    while (filled < sizeof(id)) {
      auto n = getrandom(reinterpret_cast<char *>(&id) + filled,
                         sizeof(id) - filled, 0);
      if (n < 0 && errno == EINTR)
        continue;
      if (n <= 0)
        throw std::runtime_error("OS entropy unavailable");
      filled += static_cast<size_t>(n);
    }
    return id;
  }
  void join(const Ref &p, View packet) {
    Decoder d{packet};
    auto name = d.string();
    d.done();
    if (name.size() > 16)
      throw ProtocolError("Channel Length Exceeded");
    auto &r = runtime.registry;
    std::unique_lock guard(r.mutex);
    auto room = r.channels.find(std::string(name));
    if (room == r.channels.end()) {
      if (r.channels.size() >= runtime.config.max_channels) {
        request_close(p, 1013);
        return;
      }
      room = r.channels.try_emplace(std::string(name)).first;
    }
    if (room->second.members.size() >= runtime.config.max_members) {
      request_close(p, 1013);
      return;
    }
    do {
      p->id = random_id();
    } while (p->id == 0 || p->id == broadcast || r.users.contains(p->id));
    p->channel = name;
    p->joined = true;
    r.users.emplace(p->id, p);
    room->second.members.push_back(p);
    send(p, number(p->id));
  }
  static void erase(std::vector<Ref> &v, Peer *p) {
    auto i =
        std::find_if(v.begin(), v.end(), [p](auto &x) { return x.get() == p; });
    if (i != v.end()) {
      *i = std::move(v.back());
      v.pop_back();
    }
  }
  void remove(const Ref &p) {
    p->closing.store(true, std::memory_order_relaxed);
    if (p->joined) {
      auto &r = runtime.registry;
      std::unique_lock guard(r.mutex);
      auto room = r.channels.find(p->channel);
      auto msg = envelope(broadcast, number(p->id));
      std::shared_ptr<const std::string> retained;
      if (room != r.channels.end()) {
        erase(room->second.members, p.get());
        if (!runtime.stopping.load())
          for (auto &dest : room->second.members)
            submit(dest, msg, retained);
        if (room->second.members.empty() && p->channel != global) {
          r.variables -= room->second.variable_bytes;
          r.channels.erase(room);
        }
      }
      if (p->channel == global)
        for (auto &list : r.observers)
          erase(list, p.get());
      else if (!runtime.stopping.load())
        for (auto &dest : r.observers[2])
          submit(dest, msg, retained);
      auto i = r.users.find(p->id);
      if (i != r.users.end() && i->second.get() == p.get())
        r.users.erase(i);
      ++runtime.disconnected;
    }
    runtime.buffered -= p->buffered_bytes;
    runtime.retained_bytes -= p->buffered_bytes;
    p->buffered_bytes = 0;
    --runtime.connections;
    live.erase(p.get());
    p->socket = nullptr;
  }
  void control(const Ref &p, View value) {
    Decoder d{value};
    require(d.array() == 2);
    auto op = d.integer();
    auto &r = runtime.registry;
    switch (op) {
    case 0: {
      auto password = d.string();
      d.done();
      // Fixed-work comparison for an equal-length secret; no early byte exit.
      unsigned diff = password.size() != runtime.config.auth.size();
      if (!runtime.config.auth.empty() && !diff)
        for (size_t i = 0; i < password.size(); ++i)
          diff |= static_cast<unsigned char>(password[i]) ^
                  static_cast<unsigned char>(runtime.config.auth[i]);
      if (!runtime.config.auth.empty() && !diff)
        p->authenticated = true;
      send(p, reply(0, p->authenticated ? "\xf5" : "\xf4"));
      break;
    }
    case 1: {
      auto flags = d.integer();
      require(flags <= 255);
      d.done();
      if (!p->authenticated) {
        request_close(p, 1008);
        return;
      }
      std::unique_lock guard(r.mutex);
      if (p->closing.load())
        return;
      if (p->channel == global)
        for (unsigned bit = 0; bit < 3; ++bit) {
          if ((p->flags ^ flags) & (1u << bit)) {
            if (flags & (1u << bit))
              r.observers[bit].push_back(p);
            else
              erase(r.observers[bit], p.get());
          }
        }
      p->flags = flags & 7;
      send(p, reply(1, number(p->flags)));
      break;
    }
    case 2: {
      d.null();
      d.done();
      if (!p->authenticated) {
        request_close(p, 1008);
        return;
      }
      std::vector<std::pair<std::string, size_t>> rooms;
      {
        std::shared_lock guard(r.mutex);
        rooms.reserve(r.channels.size());
        for (auto &[name, room] : r.channels)
          rooms.emplace_back(name, room.members.size());
      }
      std::sort(rooms.begin(), rooms.end());
      std::string out;
      encode(out, 4, rooms.size());
      for (auto &[name, count] : rooms) {
        out += '\x82';
        binary(out, name);
        encode(out, 0, count);
      }
      send(p, reply(2, out));
      break;
    }
    case 3: {
      auto id = d.integer();
      d.done();
      if (!p->authenticated) {
        request_close(p, 1008);
        return;
      }
      if (!id || id == broadcast) {
        request_close(p, 1008, CloseReason::Reserved);
        return;
      }
      std::unique_lock guard(r.mutex);
      if (p->closing.load())
        return;
      auto owner = r.users.find(p->id);
      if (owner == r.users.end() || owner->second.get() != p.get())
        return;
      if (id != p->id) {
        r.users.erase(owner);
        auto previous = r.users.find(id);
        if (previous != r.users.end()) {
          auto displaced = previous->second;
          displaced->owner->request_close(displaced, 4001);
          previous->second = p;
        } else
          r.users.emplace(id, p);
        p->id = id;
      }
      send(p, reply(3, number(id)));
      break;
    }
    case 4: {
      require(d.array() == 2);
      auto key = d.string();
      require(!key.empty() && key.size() <= 255);
      auto stored = d.value();
      d.done();
      std::unique_lock guard(r.mutex);
      if (p->closing.load())
        return;
      auto &room = r.channels.at(p->channel);
      auto i = room.variables.find(std::string(key));
      size_t old = i == room.variables.end()
                       ? 0
                       : i->first.size() + i->second.size(),
             next = key.size() + stored.size();
      auto &c = runtime.config;
      if (room.variable_bytes - old + next > c.variable_bytes ||
          r.variables - old + next > c.total_variable_bytes ||
          (!old && room.variables.size() >= c.max_variables)) {
        request_close(p, 1013);
        return;
      }
      room.variables[std::string(key)] = stored;
      room.variable_bytes = room.variable_bytes - old + next;
      r.variables = r.variables - old + next;
      send(p, reply(4, "\xf5"));
      break;
    }
    case 5: {
      auto key = d.string();
      require(!key.empty() && key.size() <= 255);
      d.done();
      std::shared_lock guard(r.mutex);
      auto &vars = r.channels.at(p->channel).variables;
      auto i = vars.find(std::string(key));
      std::string out("\x82", 1);
      out += i == vars.end() ? '\xf4' : '\xf5';
      out += i == vars.end() ? View("\xf6") : View(i->second);
      send(p, reply(200, out));
      break;
    }
    default:
      throw ProtocolError("Protocol Violation");
    }
  }
  void message(const Ref &p, View packet) {
    if (runtime.stopping.load(std::memory_order_relaxed))
      return;
    if (p->closing.load(std::memory_order_relaxed))
      return;
    if (!p->joined) {
      join(p, packet);
      return;
    }
    Decoder d{packet};
    require(d.array() == 2);
    auto target = d.integer();
    auto value = d.value();
    d.done();
    ++runtime.sent;
    runtime.value_bytes += value.size();
    if (!target) {
      control(p, value);
      return;
    }
    auto &r = runtime.registry;
    std::shared_lock guard(r.mutex);
    if (p->closing.load(std::memory_order_relaxed))
      return;
    if (target == broadcast && p->channel == global &&
        runtime.config.restrict_global && !p->authenticated) {
      request_close(p, 1008, CloseReason::Global);
      return;
    }
    // uWS owns writable receive storage for this callback. Reuse only the
    // parsed prefix region; the opaque validated value stays byte-for-byte
    // unchanged.
    std::string prefix;
    prefix += '\x82';
    encode(prefix, 0, p->id);
    View out;
    if (size_t(value.data() - packet.data()) >= prefix.size()) {
      char *start = const_cast<char *>(value.data()) - prefix.size();
      std::copy(prefix.begin(), prefix.end(), start);
      out = View(start, prefix.size() + value.size());
      runtime.borrowed += value.size();
    } else {
      scratch.assign(prefix);
      scratch.append(value);
      out = scratch;
      runtime.copied += value.size();
    }
    std::shared_ptr<const std::string> retained;
    if (target == broadcast) {
      if (p->channel == global) {
        for (auto &[id, dest] : r.users) {
          (void)id;
          if (dest.get() != p.get())
            submit(dest, out, retained);
        }
      } else {
        for (auto &dest : r.channels.at(p->channel).members)
          if (dest.get() != p.get())
            submit(dest, out, retained);
        for (auto &dest : r.observers[0])
          submit(dest, out, retained);
      }
    } else {
      auto i = r.users.find(target);
      if (i == r.users.end())
        return;
      submit(i->second, out, retained);
      for (auto &dest : r.observers[1])
        if (dest.get() != i->second.get())
          submit(dest, out, retained);
    }
  }
  void run(std::barrier<> &ready) {
    std::string name = "war-cpp-" + std::to_string(index);
    pthread_setname_np(pthread_self(), name.c_str());
    if (runtime.config.pin) {
      cpu_set_t allowed;
      CPU_ZERO(&allowed);
      if (pthread_getaffinity_np(pthread_self(), sizeof(allowed), &allowed))
        runtime.startup_failed = true;
      std::vector<int> cpus;
      for (int cpu = 0; cpu < CPU_SETSIZE; ++cpu)
        if (CPU_ISSET(cpu, &allowed))
          cpus.push_back(cpu);
      if (index >= cpus.size())
        runtime.startup_failed = true;
      else {
        cpu_set_t single;
        CPU_ZERO(&single);
        CPU_SET(cpus[index], &single);
        if (pthread_setaffinity_np(pthread_self(), sizeof(single), &single))
          runtime.startup_failed = true;
      }
    }
    loop = uWS::Loop::get();
    uWS::App app;
    application = &app;
    keepalive = us_create_timer(reinterpret_cast<us_loop_t *>(loop), 0, 0);
    us_timer_set(keepalive, [](auto *) {}, 1000, 1000);
    auto &c = runtime.config;
    app.ws<SocketData>(
        "/*", {.compression = uWS::DISABLED,
               .maxPayloadLength = static_cast<unsigned>(c.max_message),
               .idleTimeout = static_cast<unsigned short>(c.idle_seconds),
               .maxBackpressure = static_cast<unsigned>(
                   std::min(c.queue_bytes, size_t(UINT32_MAX))),
               .closeOnBackpressureLimit = false,
               .resetIdleTimeoutOnSend = false,
               .sendPingsAutomatically = false,
               .open =
                   [this](Socket *ws) {
                     if (runtime.stopping.load()) {
                       ws->end(1001, "Server Shutdown");
                       return;
                     }
                     if (!runtime.reserve(runtime.connections, 1,
                                          runtime.config.max_connections)) {
                       ++runtime.rejected;
                       ws->end(1013, "Try Again Later");
                       return;
                     }
                     auto p = std::make_shared<Peer>(this);
                     p->socket = ws;
                     ws->getUserData()->peer = p;
                     live.emplace(p.get(), p);
                     auto fd = static_cast<int>(
                         reinterpret_cast<intptr_t>(ws->getNativeHandle()));
                     auto set_buffer = [&](int option, unsigned bytes) {
                       if (!bytes)
                         return true;
                       int value = static_cast<int>(bytes);
                       return setsockopt(fd, SOL_SOCKET, option, &value,
                                         sizeof(value)) == 0;
                     };
                     if (!set_buffer(SO_SNDBUF,
                                     runtime.config.tcp_send_buffer_bytes) ||
                         !set_buffer(SO_RCVBUF,
                                     runtime.config.tcp_receive_buffer_bytes))
                       request_close(p, 1013);
                   },
               .message =
                   [this](Socket *ws, View bytes, uWS::OpCode op) {
                     auto p = ws->getUserData()->peer;
                     if (!p)
                       return;
                     if (op != uWS::BINARY) {
                       request_close(p, 1003);
                       return;
                     }
                     try {
                       message(p, bytes);
                     } catch (const ProtocolError &e) {
                       ++runtime.protocol_errors;
                       p->closing = true;
                       ws->end(1002, e.what());
                     }
                   },
               .drain =
                   [this](Socket *ws) {
                     auto p = ws->getUserData()->peer;
                     if (p)
                       reconcile_buffer(p);
                   },
               .ping =
                   [this](Socket *ws, View) {
                     auto p = ws->getUserData()->peer;
                     if (p)
                       reconcile_buffer(p);
                   },
               .close =
                   [this](Socket *ws, int, View) {
                     auto p = ws->getUserData()->peer;
                     if (p)
                       remove(p);
                   }});
    if (!index && !c.no_admin) {
      app.get("/readyz", [](auto *res, auto *) { res->end("ready\n"); });
      app.get("/metrics", [this](auto *res, auto *req) {
        if (runtime.config.admin_token.empty() ||
            req->getHeader("authorization") !=
                "Bearer " + runtime.config.admin_token) {
          res->writeStatus("401 Unauthorized")->end();
          return;
        }
        res->writeHeader("Content-Type", "text/plain")->end(runtime.metrics());
      });
      // Admin routes share this worker's loop, on a distinct loopback listener.
      app.listen("127.0.0.1", c.admin_port,
                 [this](auto *s) { admin_listener = s; });
    }
    app.listen("127.0.0.1", c.port, [this](auto *s) { listener = s; });
    if (!listener || (!index && !c.no_admin && !admin_listener))
      runtime.startup_failed = true;
    ready.arrive_and_wait();
    app.run();
  }
  void quiesce() {
    // Completion of this callback proves this loop finished any in-flight
    // producer.
    loop->defer(
        [this] { runtime.quiesced.fetch_add(1, std::memory_order_release); });
  }
  void shutdown() {
    loop->defer([this] {
      application
          ->close(); // includes incomplete HTTP upgrades and listen sockets
      std::lock_guard guard(inbox_mutex);
      auto release = [this](Delivery &d) {
        if (d.body) {
          d.peer->pending_bytes -= d.body->size();
          --d.peer->pending_messages;
          runtime.queued -= d.body->size();
          runtime.retained_bytes -= d.body->size();
        }
      };
      for (auto &d : inbox)
        release(d);
      for (size_t i = consume_position; i < consuming.size(); ++i)
        release(consuming[i]);
      inbox.clear();
      consuming.clear();
      consume_position = 0;
      us_timer_close(keepalive);
    });
  }
};
std::string Runtime::metrics() {
  std::shared_lock guard(registry.mutex);
  std::string out;
  auto metric = [&](View key, uint64_t n) {
    out += "war_";
    out += key;
    out += ' ';
    out += std::to_string(n);
    out += '\n';
  };
  metric("connections", connections);
  metric("users", registry.users.size());
  metric("channels", registry.channels.size());
  metric("variable_bytes", registry.variables);
  metric("queued_bytes", queued);
  metric("retained_output_bytes", retained_bytes);
  metric("buffered_bytes", buffered);
  metric("sent_messages_total", sent);
  metric("deliveries_total", deliveries);
  metric("value_bytes_total", value_bytes);
  metric("disconnected_total", disconnected);
  metric("protocol_errors_total", protocol_errors);
  metric("slow_consumers_total", slow);
  metric("rejected_total", rejected);
  metric("borrowed_value_bytes_total", borrowed);
  metric("copied_bytes_total", copied);
  return out;
}
Config parse_cli(int argc, char **argv) {
  Config c;
  if (auto s = std::getenv("WAR_AUTH_PASSWORD"))
    c.auth = s;
  if (auto s = std::getenv("WAR_ADMIN_TOKEN"))
    c.admin_token = s;
  for (int i = 1; i < argc; ++i) {
    View arg = argv[i];
    if (arg == "--no-admin") {
      c.no_admin = true;
      continue;
    }
    if (arg == "--pin-workers") {
      c.pin = true;
      continue;
    }
    if (arg == "--disable-global-broadcast") {
      c.restrict_global = true;
      continue;
    }
    if (arg == "--help") {
      std::cout
          << "WAR modern C++ experimental relay\n--bind 127.0.0.1:PORT "
             "--admin-bind 127.0.0.1:PORT --workers N "
             "--pin-workers\n--max-connections N --max-channel-members N "
             "--max-channels N --max-message-bytes N\n--queue-messages N "
             "--queue-bytes N --total-queue-bytes N\n--max-variables N "
             "--max-variable-bytes N --total-variable-bytes N\n--no-admin "
             "--disable-global-broadcast\nSecrets: WAR_AUTH_PASSWORD / "
             "WAR_ADMIN_TOKEN. Plaintext loopback only.\n";
      std::exit(0);
    }
    if (++i == argc)
      throw std::runtime_error("missing option value");
    View value = argv[i];
    if (arg == "--bind" || arg == "--admin-bind") {
      require(value.starts_with("127.0.0.1:"));
      auto n = std::stoul(std::string(value.substr(10)));
      require(n && n <= 65535);
      (arg == "--bind" ? c.port : c.admin_port) = n;
      continue;
    }
    size_t used = 0;
    auto n = std::stoull(std::string(value), &used);
    require(used == value.size());
    if (arg == "--workers")
      c.workers = n;
    else if (arg == "--tcp-send-buffer-bytes") {
      require(n <= INT32_MAX);
      c.tcp_send_buffer_bytes = n;
    } else if (arg == "--tcp-receive-buffer-bytes") {
      require(n <= INT32_MAX);
      c.tcp_receive_buffer_bytes = n;
    } else if (arg == "--max-connections")
      c.max_connections = n;
    else if (arg == "--max-channels")
      c.max_channels = n;
    else if (arg == "--max-channel-members")
      c.max_members = n;
    else if (arg == "--max-message-bytes")
      c.max_message = n;
    else if (arg == "--queue-messages")
      c.queue_messages = n;
    else if (arg == "--queue-bytes")
      c.queue_bytes = n;
    else if (arg == "--total-queue-bytes")
      c.total_queue_bytes = n;
    else if (arg == "--max-variables")
      c.max_variables = n;
    else if (arg == "--max-variable-bytes")
      c.variable_bytes = n;
    else if (arg == "--total-variable-bytes")
      c.total_variable_bytes = n;
    else
      throw std::runtime_error("unsupported option: " + std::string(arg));
  }
  require(c.workers > 0 && c.workers <= CPU_SETSIZE && c.max_connections &&
          c.max_channels && c.max_members && c.max_message &&
          c.max_message <= UINT32_MAX && c.queue_messages &&
          c.queue_bytes >= c.max_message + 10 &&
          c.total_queue_bytes >= c.queue_bytes);
  return c;
}
void run(Config config, std::stop_token stop) {
  require(config.workers > 0 && config.workers <= CPU_SETSIZE &&
          config.max_connections && config.max_channels && config.max_members &&
          config.max_message && config.max_message <= UINT32_MAX &&
          config.queue_messages &&
          config.queue_bytes >= config.max_message + 10 &&
          config.total_queue_bytes >= config.queue_bytes);
  Runtime runtime(std::move(config));
  std::barrier ready(runtime.config.workers + 1);
  std::vector<std::thread> threads;
  for (unsigned i = 0; i < runtime.config.workers; ++i)
    runtime.workers.push_back(std::make_unique<Worker>(runtime, i));
  for (auto &w : runtime.workers)
    threads.emplace_back([&ready, p = w.get()] { p->run(ready); });
  ready.arrive_and_wait();
  std::cout << "WAR C++ ready workers=" << runtime.config.workers << std::endl;
  while (!stop.stop_requested() && !runtime.startup_failed.load())
    std::this_thread::sleep_for(std::chrono::milliseconds(10));
  runtime.stopping = true;
  for (auto &w : runtime.workers)
    w->quiesce();
  while (runtime.quiesced.load(std::memory_order_acquire) !=
         runtime.config.workers)
    std::this_thread::sleep_for(std::chrono::milliseconds(1));
  for (auto &w : runtime.workers)
    w->shutdown();
  for (auto &t : threads)
    t.join();
  if (runtime.queued != 0 || runtime.connections != 0 ||
      runtime.retained_bytes != 0)
    throw std::runtime_error("nonzero shutdown resources");
  if (runtime.startup_failed)
    throw std::runtime_error("listener bind or worker affinity setup failed");
}
} // namespace war
