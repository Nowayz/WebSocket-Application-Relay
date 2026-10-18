#include "../relay.hpp"
#include <stop_token>
int main() {
  for (int n = 0; n < 3; ++n) {
    war::Config config;
    config.port = 0;
    config.no_admin = true;
    config.workers = 2;
    std::stop_source stop;
    stop.request_stop();
    war::run(std::move(config), stop.get_token());
  }
}
