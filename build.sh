#!/bin/sh
clang++ -O3 -march=native -mrdrnd -fomit-frame-pointer -std=c++1z -o app relay.cpp uws/Extensions.cpp uws/Group.cpp uws/HTTPSocket.cpp uws/Hub.cpp uws/WebSocket.cpp uws/WebSocketImpl.cpp uws/Networking.cpp uws/Node.cpp uws/Socket.cpp uws/uUV.cpp -Iuws -ltbb -lz -pthread -lssl -luv -lcrypto
