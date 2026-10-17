#pragma once
#include <array>
#include <cstdint>
#include <stdexcept>
#include <string>
#include <string_view>

namespace war {
struct ProtocolError : std::runtime_error {
  using runtime_error::runtime_error;
};
using View = std::string_view;
inline void require(bool condition) {
  if (!condition)
    throw ProtocolError("Protocol Violation");
}
inline bool utf8(View s) {
  for (size_t i = 0; i < s.size();) {
    auto c = static_cast<unsigned char>(s[i++]);
    if (c < 128)
      continue;
    unsigned n;
    uint32_t cp, minimum;
    if (c >= 0xc2 && c <= 0xdf) {
      n = 1;
      cp = c & 31;
      minimum = 128;
    } else if (c >= 0xe0 && c <= 0xef) {
      n = 2;
      cp = c & 15;
      minimum = 2048;
    } else if (c >= 0xf0 && c <= 0xf4) {
      n = 3;
      cp = c & 7;
      minimum = 65536;
    } else
      return false;
    if (n > s.size() - i)
      return false;
    while (n--) {
      auto d = static_cast<unsigned char>(s[i++]);
      if ((d & 192) != 128)
        return false;
      cp = (cp << 6) | (d & 63);
    }
    if (cp < minimum || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff))
      return false;
  }
  return true;
}
struct Decoder {
  View bytes;
  size_t pos = 0;
  unsigned byte() {
    require(pos < bytes.size());
    return static_cast<unsigned char>(bytes[pos++]);
  }
  struct Head {
    unsigned major, additional;
    uint64_t value;
  };
  Head head() {
    unsigned b = byte(), a = b & 31;
    require(a < 28);
    uint64_t v = a;
    if (a >= 24) {
      unsigned n = 1u << (a - 24);
      require(n <= bytes.size() - pos);
      v = 0;
      while (n--)
        v = (v << 8) | byte();
    }
    return {b >> 5, a, v};
  }
  uint64_t integer() {
    auto h = head();
    require(h.major == 0);
    return h.value;
  }
  uint64_t array() {
    auto h = head();
    require(h.major == 4);
    return h.value;
  }
  View string() {
    auto h = head();
    require(h.major == 2 || h.major == 3);
    require(h.value <= bytes.size() - pos);
    auto s = bytes.substr(pos, h.value);
    pos += h.value;
    require(h.major != 3 || utf8(s));
    return s;
  }
  void null() { require(byte() == 0xf6); }
  void done() { require(pos == bytes.size()); }
  View value() {
    size_t start = pos, depth = 0;
    uint64_t items = 0;
    std::array<uint64_t, 65> pending{};
    pending[0] = 1;
    for (;;) {
      if (!pending[depth]) {
        if (!depth)
          return bytes.substr(start, pos - start);
        --depth;
        continue;
      }
      --pending[depth];
      require(++items <= 65536);
      auto h = head();
      uint64_t children = 0;
      switch (h.major) {
      case 0:
      case 1:
        break;
      case 2:
      case 3: {
        require(h.value <= bytes.size() - pos);
        auto s = bytes.substr(pos, h.value);
        pos += h.value;
        require(h.major != 3 || utf8(s));
        break;
      }
      case 4:
        children = h.value;
        break;
      case 5:
        require(h.value <= UINT64_MAX / 2);
        children = h.value * 2;
        break;
      case 6:
        children = 1;
        break;
      case 7:
        require(h.additional != 24 || h.value >= 32);
        break;
      }
      if (children) {
        require(depth < 64 && children <= 65536 - items &&
                children <= bytes.size() - pos);
        pending[++depth] = children;
      }
    }
  }
};
inline void encode(std::string &out, unsigned major, uint64_t value) {
  if (value < 24) {
    out.push_back(static_cast<char>((major << 5) | value));
    return;
  }
  unsigned n = value <= 255          ? 1
               : value <= 65535      ? 2
               : value <= UINT32_MAX ? 4
                                     : 8;
  out.push_back(static_cast<char>((major << 5) | (n == 1   ? 24
                                                  : n == 2 ? 25
                                                  : n == 4 ? 26
                                                           : 27)));
  for (unsigned i = n; i; --i)
    out.push_back(static_cast<char>(value >> (8 * (i - 1))));
}
inline std::string number(uint64_t value) {
  std::string out;
  encode(out, 0, value);
  return out;
}
inline void binary(std::string &out, View value) {
  encode(out, 2, value.size());
  out.append(value);
}
inline std::string envelope(uint64_t sender, View value) {
  std::string out;
  out.reserve(10 + value.size());
  out.push_back('\x82');
  encode(out, 0, sender);
  out.append(value);
  return out;
}
inline std::string reply(unsigned op, View value) {
  std::string out("\x82\x00\x82", 3);
  encode(out, 0, op);
  out.append(value);
  return out;
}
} // namespace war
