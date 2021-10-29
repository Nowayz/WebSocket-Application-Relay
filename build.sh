#!/bin/sh
clang++ -O3 -march=native -mrdrnd -fomit-frame-pointer -std=c++1z -o app relay.cpp uws/Socket.cpp uws/WebSocket.cpp uws/Room.cpp uws/Node.cpp uws/Networking.cpp uws/Hub.cpp uws/HTTPSocket.cpp uws/Group.cpp uws/Extensions.cpp uws/Epoll.cpp -Iuws -ltbb -lz -pthread -lssl -luv -lcrypto
