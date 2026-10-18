#include "relay.hpp"
#include <atomic>
#include <chrono>
#include <csignal>
#include <iostream>
#include <thread>
namespace {
std::atomic<bool> interrupted{false};
static_assert(std::atomic<bool>::is_always_lock_free);
void signal_handler(int) { interrupted.store(true, std::memory_order_relaxed); }
} // namespace
int main(int argc, char **argv) {
  try {
    auto config = war::parse_cli(argc, argv);
    std::signal(SIGINT, signal_handler);
    std::signal(SIGTERM, signal_handler);
    std::stop_source stop;
    std::jthread monitor([&](std::stop_token done) {
      while (!done.stop_requested() &&
             !interrupted.load(std::memory_order_relaxed))
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
      if (interrupted.load(std::memory_order_relaxed))
        stop.request_stop();
    });
    war::run(std::move(config), stop.get_token());
    return 0;
  } catch (const std::exception &e) {
    std::cerr << e.what() << '\n';
    return 1;
  }
}
