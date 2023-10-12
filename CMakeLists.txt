cmake_minimum_required(VERSION 3.10.2)

set(PROJECT_NAME relay)
project("${PROJECT_NAME}" VERSION 1.0.0)

set(CMAKE_C_STANDARD 17)
set(CMAKE_CXX_STANDARD 17)

set(SRC_INCL_DIRS
    "uws/"
)

set(SRC_FILES
    relay.cpp
    uws/Socket.cpp
    uws/WebSocket.cpp
    uws/Room.cpp
    uws/Node.cpp
    uws/Networking.cpp
    uws/Hub.cpp
    uws/HTTPSocket.cpp
    uws/Group.cpp
    uws/Extensions.cpp
    uws/Epoll.cpp
)

# Dependencies from pkg-config:
#  - tbb
#  - libuv
#  - openssl
#  - zlib

find_package(PkgConfig REQUIRED)
pkg_check_modules(TBB REQUIRED IMPORTED_TARGET tbb)
pkg_check_modules(LIBUV REQUIRED IMPORTED_TARGET libuv)
pkg_check_modules(OPENSSL REQUIRED IMPORTED_TARGET openssl)
pkg_check_modules(ZLIB REQUIRED IMPORTED_TARGET zlib)

add_executable("${PROJECT_NAME}" ${SRC_FILES})
target_include_directories("${PROJECT_NAME}" PRIVATE ${SRC_INCL_DIRS})
target_link_libraries("${PROJECT_NAME}" PRIVATE PkgConfig::TBB PkgConfig::LIBUV PkgConfig::OPENSSL PkgConfig::ZLIB)
