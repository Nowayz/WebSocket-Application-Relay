#pragma once
#include <cstddef>
#include <stop_token>
#include <string>

namespace war {
struct Config {
  unsigned port = 1338, admin_port = 1339, workers = 4;
  size_t max_connections = 10000, max_channels = 10000, max_members = 10000;
  size_t max_message = 16 * 1024 * 1024, queue_bytes = 32 * 1024 * 1024,
         queue_messages = 64;
  size_t total_queue_bytes = 256 * 1024 * 1024, max_variables = 256;
  size_t variable_bytes = 1024 * 1024, total_variable_bytes = 64 * 1024 * 1024;
  unsigned idle_seconds = 0;
  unsigned tcp_send_buffer_bytes = 0, tcp_receive_buffer_bytes = 0;
  bool pin = false, no_admin = false, restrict_global = false;
  std::string auth, admin_token;
};

// Owns worker loops until stop is requested; all workers are joined before
// return. Bind failures and violated runtime invariants are fatal in this
// experimental API.
void run(Config config, std::stop_token stop);
Config parse_cli(int argc, char **argv);
} // namespace war
