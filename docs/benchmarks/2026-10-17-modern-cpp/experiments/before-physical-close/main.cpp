#include "relay.hpp"
#include <chrono>
#include <csignal>
#include <iostream>
#include <thread>
namespace {
volatile std::sig_atomic_t interrupted = 0;
void signal_handler(int) { interrupted = 1; }
} // namespace
int main(int argc, char **argv) {
  try {
    auto config = war::parse_cli(argc, argv);
    std::signal(SIGINT, signal_handler);
    std::signal(SIGTERM, signal_handler);
    std::stop_source stop;
    std::jthread monitor([&](std::stop_token done) {
      while (!done.stop_requested() && !interrupted)
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
      if (interrupted)
        stop.request_stop();
    });
    war::run(std::move(config), stop.get_token());
    return 0;
  } catch (const std::exception &e) {
    std::cerr << e.what() << '\n';
    return 1;
  }
}
