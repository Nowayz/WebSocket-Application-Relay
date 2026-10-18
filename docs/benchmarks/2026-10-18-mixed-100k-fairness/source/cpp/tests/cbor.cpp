#include "../cbor.hpp"
#include <iostream>
int main() {
  using namespace war;
  auto check = [](bool b) {
    if (!b)
      throw std::runtime_error("test failed");
  };
  for (auto n :
       {uint64_t(0), uint64_t(23), uint64_t(24), uint64_t(255), uint64_t(256),
        uint64_t(65535), uint64_t(65536), uint64_t(UINT32_MAX), UINT64_MAX}) {
    auto s = number(n);
    Decoder d{s};
    check(d.integer() == n);
    d.done();
  }
  for (unsigned n = 0; n < 256; ++n) {
    std::string s = "\xf8";
    s.push_back(static_cast<char>(n));
    bool valid = true;
    try {
      Decoder d{s};
      d.value();
      d.done();
    } catch (const ProtocolError &) {
      valid = false;
    }
    check(valid == (n >= 32));
  }
  for (unsigned depth : {0, 1, 64, 65}) {
    std::string s(depth, '\x81');
    s += '\xf6';
    bool valid = true;
    try {
      Decoder d{s};
      d.value();
      d.done();
    } catch (const ProtocolError &) {
      valid = false;
    }
    check(valid == (depth <= 64));
  }
  check(utf8("\xe2\x82\xac"));
  check(!utf8("\xed\xa0\x80"));
  check(!utf8("\xc0\xaf"));
  check(!utf8("\xf4\x90\x80\x80"));
  for (auto s : {std::string("\x9f"), std::string("\x61\xff"),
                 std::string("\x5b\xff\xff\xff\xff\xff\xff\xff\xff")}) {
    bool rejected = false;
    try {
      Decoder d{s};
      d.value();
      d.done();
    } catch (const ProtocolError &) {
      rejected = true;
    }
    check(rejected);
  }
  std::cout << "CBOR boundary checks passed\n";
}
