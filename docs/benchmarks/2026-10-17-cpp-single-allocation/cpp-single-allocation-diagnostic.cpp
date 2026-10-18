#undef NDEBUG
#include <atomic>
#include <cassert>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <memory>
#include <new>
#include <string>
#include <string_view>
using View = std::string_view;
static thread_local bool track = false;
static thread_local size_t calls = 0, allocated = 0;
__attribute__((noinline)) void *operator new(size_t n) {
  if (track) { ++calls; allocated += n; }
  if (void *p = std::malloc(n ? n : 1)) return p;
  throw std::bad_alloc();
}
void *operator new[](size_t n) { return ::operator new(n); }
void operator delete(void *p) noexcept { std::free(p); }
void operator delete[](void *p) noexcept { std::free(p); }
void operator delete(void *p, size_t) noexcept { std::free(p); }
void operator delete[](void *p, size_t) noexcept { std::free(p); }
struct OwnedMessage {
  std::shared_ptr<const char[]> body;
  size_t length = 0;
  static OwnedMessage copy(View value) {
    auto bytes = std::make_shared_for_overwrite<char[]>(value.size());
    if (!value.empty())
      std::memcpy(bytes.get(), value.data(), value.size());
    return {std::move(bytes), value.size()};
  }
};

__attribute__((noinline)) OwnedMessage make_current(View input) {
  auto message = OwnedMessage::copy(input);
  asm volatile("" : : "r"(message.body.get()) : "memory");
  return message;
}
__attribute__((noinline)) std::shared_ptr<const std::string> make_old(View input) {
  auto message = std::make_shared<const std::string>(input);
  asm volatile("" : : "r"(message.get()) : "memory");
  return message;
}
int main() {
  std::cout << "[";
  bool first = true;
  for (size_t size : {size_t(0), size_t(1), size_t(12), size_t(15), size_t(16), size_t(64), size_t(1037), size_t(65536)}) {
    std::string input(size, '\0');
    for (size_t i = 0; i < size; ++i) input[i] = char(i * 37);
    size_t old_calls, old_bytes, new_calls, new_bytes;
    calls = allocated = 0; track = true;
    auto old = make_old(input);
    track = false; old_calls = calls; old_bytes = allocated;
    assert(*old == input);
    calls = allocated = 0; track = true;
    auto current = make_current(input);
    track = false; new_calls = calls; new_bytes = allocated;
    assert(View(current.body.get(), current.length) == input);
    auto share = current;
    assert(share.body.get() == current.body.get());
    current = {};
    assert(View(share.body.get(), share.length) == input);
    assert(new_calls == 1);
    if (size > 15) assert(old_calls == 2);
    if (!first) std::cout << ",";
    first = false;
    std::cout << "{\"body_bytes\":" << size << ",\"control_allocations\":" << old_calls
              << ",\"candidate_allocations\":" << new_calls << ",\"control_allocated_bytes\":"
              << old_bytes << ",\"candidate_allocated_bytes\":" << new_bytes << "}";
  }
  std::cout << "]\n";
}
