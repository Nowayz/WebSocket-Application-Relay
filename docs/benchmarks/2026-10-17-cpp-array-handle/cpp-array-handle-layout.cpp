#undef NDEBUG
#include <cassert>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <memory>
#include <string_view>
using View=std::string_view;
struct Peer;
using Ref=std::shared_ptr<Peer>;
enum class CloseReason : unsigned char { Automatic, Reserved, Global };
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
using RetainedBody = std::shared_ptr<const char[]>;
static RetainedBody retain_body(View value) {
  auto bytes = std::make_shared_for_overwrite<char[]>(value.size());
  if (!value.empty())
    std::memcpy(bytes.get(), value.data(), value.size());
  return bytes;
}
struct Delivery {
  Ref peer;
  RetainedBody body;
  struct Close {
    uint16_t code;
    CloseReason reason;
  };
  // A non-null body selects length; a close has no body and selects close.
  // These alternatives never overlap in use. Reusing the existing tail keeps
  // a full size_t length without growing every recipient's queue record.
  union {
    size_t length;
    Close close;
  };
  Delivery(Ref p, const RetainedBody &message, size_t size)
      : peer(std::move(p)), body(message), length(size) {}
  Delivery(Ref p, uint16_t code, CloseReason reason)
      : peer(std::move(p)), close{code, reason} {}
};
static_assert(sizeof(Delivery) == 40);
int main() {
 char input[]={'a','b','c',0,'d','e','f'};
 auto bytes=retain_body(View(input,sizeof input));
 input[0]='z';
 assert(bytes[0]=='a');
 auto shared=bytes;bytes.reset();
 assert(View(shared.get(),sizeof input)==View("abc\0def",7));
 std::cout << "{\"retained_handle_bytes\":" << sizeof(RetainedBody)
 << ",\"previous_wrapper_bytes\":" << sizeof(OwnedMessage)
 << ",\"delivery_bytes\":" << sizeof(Delivery) << ",\"ownership_checked\":true}\n";
}
