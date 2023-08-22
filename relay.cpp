#include <stdint.h>
#include <vector>
#include <thread>
#include <chrono>
#include <uWS.h>
#include <ctime>
#include <atomic>

// Don't include immintrin for ARM64 builds
#if defined(__x86_64__) || defined(_M_X64)
	#include <immintrin.h>
#endif

#include "tbb/tbb.h"
#include "tbb/concurrent_unordered_map.h"
#include "tbb/concurrent_queue.h"
#include "tbb/concurrent_unordered_set.h"

//
// SERVER CONFIGURATION SETTINGS
//
#define SERVER_PORT 1338
#define SERVER_TLS_CERTIFICATE "/etc/letsencrypt/live/gl.ax/cert.pem"
#define SERVER_TLS_PRIVATEKEY  "/etc/letsencrypt/live/gl.ax/privkey.pem"
#define SERVER_TLS_KEYPASSWORD ""


// GARBAGE COLLECTION
#define WAIT_FOR_GC() while (gc_State==-1) {continue;}
#define WAIT_FOR_RELAY() while (gc_State>0) {continue;}

// WEBSOCKET EXIT CODES (and Messages)
#define CLOSE_PROTOCOL_ERROR  1002
#define CLOSE_UNSUPPORTED     1003
#define CLOSE_TRY_AGAIN_LATER 1013
#define CLOSE_USERID_TAKEN    4001  // Custom

#define MSG_PROTOCOL_VIOLATION      "Protocol Violation"
#define MSG_TYPE_UNSUPPORTED        "Type Unsupported"
#define MSG_CHANNEL_LENGTH_EXCEEDED "Channel Length Exceeded"
#define MSG_USERID_TAKEN            "UserID Taken"

// FIXED USERID TARGETS
#define RE_BROADCAST_TARGET 0xFFFFFFFFFFFFFFFF  // Broadcasts to everybody in the channel
#define RE_RELAY_TARGET     0x0000000000000000  // Allows interfacing with the relay itself

// WINDOWS LINKER
#ifdef _WIN32
#include <io.h>
#define read _read

#pragma comment(lib, "libcryptoMT.lib")
#pragma comment(lib, "libsslMT.lib")
#pragma comment(lib, "zlib.lib")
#pragma comment(lib, "libuv.lib")
#pragma comment(lib, "Iphlpapi.lib")
#pragma comment(lib, "psapi.lib")
#pragma comment(lib, "Crypt32.lib")
#pragma comment(lib, "Userenv.lib")
#pragma comment(lib, "ws2_32.lib")
#endif

/////////////////////
// GLOBALS
/////////////////
struct Session;
std::atomic<char> gc_State; // garbage collector state

// LOOKUP TABLES
tbb::concurrent_unordered_map<uint64_t, Session*>                                   UserIDSessionMap;    //  Relay UserID  :  Session Pointer               (This was added to avoid using memory addresses as UserIDs) (Now UserID can be anything)
tbb::concurrent_unordered_set<Session*>                                             SessionExists;       //       Session  :  Is Session Pointer Valid?     (Used to confirm Point-To-Point message recepient validity)
tbb::concurrent_unordered_map<std::string, tbb::concurrent_unordered_set<Session*>> ChannelClientTable;  //  Channel Name  :  List of Subscribed Clients
tbb::concurrent_unordered_set<Session*>* reGlobalChannelIndex;

// CHANNEL VARIABLE TABLE
tbb::concurrent_unordered_map<std::string, tbb::concurrent_unordered_map<std::string, std::string>> ChannelVariables;

// GARBAGE COLLECTION QUEUE
tbb::concurrent_queue<Session*> GarbageQueue;


/////////////////////
// Auxiliary Functions
/////////////////

// 2^128-1 period
uint64_t rand64p() {
	static uint64_t a=1337, b=1338;
	static int init = 1;
	uint64_t x = a; a = b;
	return a + (b = (x ^= x << 23) ^ b ^ (x >> 17) ^ (b >> 26));
}

int enc64(const char* input, int len, char* output) {
	static const char b64[] =
		"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
	int i;

	if (len == 0) {
		return 0;
	}
	char q, *p = (char*)output;
	if (len == 1) {
		i = 0;
		*p++ = b64[(input[i] >> 2) & 0x3F];
		q = (input[i++] & 0x03) << 4;
		*p++ = b64[q | ((input[i] & 0xF0) >> 4)];
		*p++ = '=';
		*p++ = '=';
	}
	else {
		for (i = 0; i < len - 2; i++) {
			*p++ = b64[(input[i] >> 2) & 0x3F];
			q = (input[i++] & 0x03) << 4;
			*p++ = b64[q | ((input[i] & 0xF0) >> 4)];
			q = (input[i++] & 0x0F) << 2;
			*p++ = b64[q | ((input[i] & 0xC0) >> 6)];
			*p++ = b64[input[i] & 0x3F];
		}
		if (i < len) {
			*p++ = b64[(input[i] >> 2) & 0x3F];
			if (i == (len - 1)) {
				*p++ = b64[((input[i] & 0x3) << 4)];
				*p++ = '=';
			}
			else {
				q = (input[i++] & 0x3) << 4;
				*p++ = b64[q | ((input[i] & 0xF0) >> 4)];
				*p++ = b64[((input[i] & 0xF) << 2)];
			}
			*p++ = '=';
		}
	}
	return p - output;
}

static int dec64(const char* in, int len, unsigned char* out) {
	const static int T[] = {-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,
		-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,62,-1,-1,-1,
		63,52,53,54,55,56,57,58,59,60,61,-1,-1,-1,-1,-1,-1,-1,0,1,2,3,4,5,6,7,8,9,10,11,
		12,13,14,15,16,17,18,19,20,21,22,23,24,25,-1,-1,-1,-1,-1,-1,26,27,28,29,30,31,32,
		33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,-1,-1,-1,-1,-1,-1,-1,-1,
		-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,
		-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,
		-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,
		-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,
		-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1};
	int index = 0;
	int val = 0, valb = -8;
	for (int i = 0; i<len; i++) {
		if (T[in[i]] == -1) break;
		val = (val << 6) + T[in[i]];
		valb += 6;
		if (valb >= 0) {
			out[index++] = (val >> valb) & 0xFF;
			valb -= 8;
		}
	}
	return index;
}


/////////////////////
// Structures
/////////////////

// Message Mode Masks
enum {
	NoMessages        = 0b0000,
	ChannelMessage    = 0b0001,
	PrivateMessage    = 0b0010,
	DisconnectMessage = 0b0100
};

struct RelayAuth {
	const char* password;
	int   authLevel;
};

RelayAuth Credentials[] = {
	{ "metalgear", 1 }
};

/* 
		Relay Session Information
	> The goal is to be as lightweight as possible, only holding information
	needed for administration, security, or by 'Host Clients' on the relay.

	Host Clients are special relay clients which have unique jobs
	such as storing user information or negotiating matchmaking.

	There is not a seperate protocol for a Host Client, and they may fill
	various tasks depending on the needs of an application using the relay.
*/
struct Session {
	uWS::WebSocket<uWS::SERVER>* webSocket;
	std::time_t timeOfConnection;  // When connection occured (GMT+0)
	uint64_t userId;

	const std::string* channelName;                        // Name of the Channel the user is in
	tbb::concurrent_unordered_set<Session*>* channelIndex; // Pointer to channel array for user's channel
	std::atomic<bool> valid;                               // Is socket still valid (1 if ready, 0 if disconnected and pending deletion)
	int listenerMode;
	int authLevel;   // Level 1 = Relay Query & Listener Authentication

	Session(uWS::WebSocket<uWS::SERVER>* ws) {
		// Setup Session
		this->webSocket = ws;
		this->timeOfConnection = std::time(nullptr);
		this->valid        = true;
		this->listenerMode = 0;
		this->authLevel    = 0;

		// Generate values until finding an unused userId
		uint64_t tmpUserId;
		do {
			tmpUserId = rand64p();
		} while(UserIDSessionMap.count(tmpUserId));
		this->userId = tmpUserId;
		UserIDSessionMap[this->userId] = this;

		// Populate Lookup Tables
		ws->setUserData(this);
	}

	// Destructor MUST be performed while mutex is held preventing iterator access
	~Session() {
		// Erase UserId from userId->Session map
		// NOTICE: The userId will be NULL if original is requested by another Session
		if (this->userId) {
			UserIDSessionMap.unsafe_erase(this->userId);
		}

		// Erase session from the channel
		this->channelIndex->unsafe_erase(this);

		// If the channel has no remaining users, remove it
		if ((this->channelIndex->size() == 0) && !(this->channelIndex==reGlobalChannelIndex)) {
			
			// this->channelName is tied to the ClientTable entry so we make a copy of the string (not sure if required)
			auto &tmpName = *this->channelName; 
			ChannelClientTable.unsafe_erase(tmpName);
			ChannelVariables.unsafe_erase(tmpName);
		}

		// Erase session from global session list
		SessionExists.unsafe_erase(this);
	}
};


/////////////////////
// GARBAGE COLLECTION 
/////////////////
struct AcquireGarbageLock {
	AcquireGarbageLock() {
		WAIT_FOR_GC();
		gc_State++;
	}
	~AcquireGarbageLock() {
		gc_State--;
	}
};

void FreeGarbage() {
	// Busy wait until we can cleanup safely (and hope this doesn't take forever)
	WAIT_FOR_RELAY();
	gc_State = -1;

	Session* client;
	while (GarbageQueue.try_pop(client)) {
		delete client;
	}

	gc_State = 0;
}



/////////////////////
// MESSAGE PROCESSING
/////////////////
void AssignChannelVariable(uWS::WebSocket<uWS::SERVER>* ws, char* key, uint32_t key_length, char* value, uint32_t value_length) {
	Session* client = (Session*)ws->getUserData();
	std::string channelName = *client->channelName;
	auto node = ChannelVariables.find(channelName);
	if (node == ChannelVariables.end()) {
		// Create channel if it doesnt exist
		auto insert = ChannelVariables.insert(std::make_pair(channelName, tbb::concurrent_unordered_map<std::string, std::string>()));
	}
	std::string keyStr(key, key_length);
	std::string valueStr(value, value_length);
	ChannelVariables[channelName][keyStr] = valueStr;
}

const char* EmptyVariableReturnPacket = "\x00\x00\x00\x00\x00\x00\x00\x00\xC8";
void TransmitChannelVariable(uWS::WebSocket<uWS::SERVER>* ws, char* key, uint32_t key_length) {
	Session* client = (Session*)ws->getUserData();
	std::string channelName = *client->channelName;
	auto channelNode = ChannelVariables.find(channelName);
	if (channelNode == ChannelVariables.end()) {
		// Send empty packet if no channel vars exists
		ws->send(EmptyVariableReturnPacket, 9, uWS::OpCode::BINARY);
	}
	else {
		auto channelVariables = channelNode->second;
		std::string keyStr(key, key_length);
		auto channelVarNode = channelVariables.find(keyStr);
		if (channelVarNode == channelVariables.end()) {
			// Send empty packet if var not found
			ws->send(EmptyVariableReturnPacket, 9, uWS::OpCode::BINARY);
		}
		else {
			size_t returnSize = 8/*userId*/ + 1/*opcode*/ + channelVarNode->second.size();
			char* buffer = (char*)malloc(returnSize);
			char* cur = (char*)buffer;
			*(uint64_t*)cur = RE_RELAY_TARGET; cur += 8;
			*cur = (char)200; cur += 1;
			const char* valueData  = channelVarNode->second.data();
			size_t valueLength = channelVarNode->second.length();
			memcpy(cur, valueData, valueLength); cur += valueLength;
			ws->send(buffer, returnSize, uWS::OpCode::BINARY);
			free(buffer);
		}
	}
}


void DisconnectClient(Session* client, uWS::WebSocket<uWS::SERVER> *ws, int code, const char* msg, int msg_len) {
	if (client) {
		client->valid = false;
		GarbageQueue.push(client);
	}
	ws->close(code, msg, msg_len);
}


//   MessageSizeValid
// RETURNS
//     Returns true if minimum message size is enough, otherwise returns false.
// REMARKS
//     Binary messages less than 9 and text messages less than 12 are invalid.
bool MessageSizeValid(Session* client, uWS::WebSocket<uWS::SERVER> *ws, size_t length, uWS::OpCode type) {
	switch (type) {
		case uWS::OpCode::BINARY: {
			if (length <= 8) {
				DisconnectClient(client, ws, CLOSE_PROTOCOL_ERROR, MSG_PROTOCOL_VIOLATION, sizeof(MSG_PROTOCOL_VIOLATION));
				return false;
			}
			return true;
		}
		case uWS::OpCode::TEXT: {
			if (length <= 12) {
				DisconnectClient(client, ws, CLOSE_PROTOCOL_ERROR, MSG_PROTOCOL_VIOLATION, sizeof(MSG_PROTOCOL_VIOLATION));
				return false;
			}
			return true;
		}
		default: {
			// Unreachable
			return false;
		}
	}
}


// returns true on success, returns false if onMessage should terminate
bool HandleBinaryMessages(Session* client, uWS::WebSocket<uWS::SERVER> *ws, char* message, size_t length) {
	uWS::OpCode code = uWS::OpCode::BINARY;

	// Disconnect on invalid size
	if (!MessageSizeValid(client, ws, length, uWS::OpCode::BINARY)) 
		return false;

	// Read message target and send
	uint64_t *targetUserID = (uint64_t*)(&message[0]);
	switch (*targetUserID) {
		// BROADCAST MESSAGE
		case RE_BROADCAST_TARGET: {			
			// Overwrite message with sender's UserID
			*targetUserID = client->userId;

			// SPECIAL re_globl broadcast-message is sent to entire relay
			if (client->channelIndex == reGlobalChannelIndex) {
				for (auto &v : SessionExists) {
					if (!(ws == ((v->webSocket))) && v->valid) {
						((v->webSocket))->send(message, length, code);
					}
				}
			}
			else {
				// Send to just the channel
				for (auto &v : *client->channelIndex) {
					if (!(ws == ((v->webSocket))) && v->valid) {
						((v->webSocket))->send(message, length, code);
					}
				}

				// Send to users in 're_globl' channel with re_spy::channelmsg flag
				for (auto &v : *reGlobalChannelIndex) {
					if (!(ws == ((v->webSocket))) && v->valid) {
						if (v->listenerMode & ChannelMessage) { // Check global Relay Channel listening bit
							((v->webSocket))->send(message, length, code);
						}
					}
				}
			}
			break;
		}

		// RELAY INTERNAL MESSAGE
		case RE_RELAY_TARGET: {
			char msgBuffer[24];
			size_t msgLen;
			if (length < 9) { return false; }
			switch (message[8]) {
			case 0: {
				msgLen = length - 9;
				if ((msgLen > 0) && (msgLen < 24)) {
					strncpy(msgBuffer, &message[9], msgLen);
					for (auto &auth : Credentials) {
						if (msgLen == strlen(auth.password)) {
							if (!strncmp(msgBuffer, auth.password, msgLen)) {
								client->authLevel = auth.authLevel;
							}
						}
					}
				}
				break;
			}
			case 1: {
				if (length != 10) { return false; }
				if (client->authLevel == 1) {
					client->listenerMode = message[9];
				}
				break;
			}
			case 2: {
				if (length != 9) { return false; }
				if (client->authLevel == 1) {
					size_t numberOfChannels = 0;
					size_t stringTotal = 0;
					size_t payloadSize;
					for (auto &v : ChannelClientTable) {
						numberOfChannels++;
						stringTotal += v.first.length();
					}
					payloadSize = 8/*sender UserID*/ + 4/*number of channels*/ + numberOfChannels/*1-byte strlen*/ + stringTotal/*strings*/ + numberOfChannels * 4 /*4-byte population*/;
					char* buffer = (char*)malloc(payloadSize);
					char* cur = (char*)buffer;
					*(uint64_t*)cur = RE_RELAY_TARGET; cur += 8;
					*(uint32_t*)cur = numberOfChannels; cur += 4;
					for (auto &v : ChannelClientTable) {
						uint8_t strLen = (uint8_t)v.first.length();
						*(uint8_t*)cur = (uint8_t)strLen; cur++;
						v.first.copy(cur, strLen, 0); cur += strLen;
					}
					for (auto &v : ChannelClientTable) {
						*(uint32_t*)cur = v.second.size(); cur += 4;
					}
					ws->send(buffer, payloadSize, uWS::OpCode::BINARY);
					free(buffer);
				}
				break;
			}
			case 3: {
				// If a Session is authenticated, allow assignment of any untaken userId
				msgLen = length - 9;
				if (client->authLevel < 1) { return false; }
				if (msgLen != 8) { return false; }
				if (UserIDSessionMap.count(*(uint64_t*)(&message[9]))) {
					Session* tmpclient = UserIDSessionMap[*(uint64_t*)(&message[9])];
					tmpclient->userId = 0;
					if (tmpclient->valid) {
						DisconnectClient(tmpclient, tmpclient->webSocket, CLOSE_USERID_TAKEN, MSG_USERID_TAKEN, sizeof(MSG_USERID_TAKEN));
					}
				}
				UserIDSessionMap[*(uint64_t*)(&message[9])] = client;
				UserIDSessionMap.unsafe_erase(client->userId);
				client->userId = *(uint64_t*)(&message[9]);
				break;
			}
			case 4: {
				uint8_t key_length = *(uint8_t*)&message[9];
				uint32_t value_length = length - 10 - key_length;
				if (key_length && value_length >= 0) {
					AssignChannelVariable(ws, &message[10], key_length, &message[10 + key_length], value_length);
				}
				break;
			}
			case 5: {
				uint32_t key_length = length - 9;
				if (key_length) {
					TransmitChannelVariable(ws, (char*)&message[9], key_length);
				}
				break;
			}
			default:
				DisconnectClient(client, ws, CLOSE_PROTOCOL_ERROR, MSG_PROTOCOL_VIOLATION, sizeof(MSG_PROTOCOL_VIOLATION));
				break;
			}
			break;
		}

		// PRIVATE MESSAGE
		default: {
			auto targetSession = UserIDSessionMap.find((uint64_t)*(size_t*)message); // Since message[0] has the target we just dereference with size_t
			if (targetSession != UserIDSessionMap.end()) {
				*targetUserID = client->userId; // Prefix message with sender's UserID

				// Send Private Message to Target
				if ((targetSession->second)->valid) {
					(((targetSession->second)->webSocket))->send(message, length, code);
				}

				// Send Private Message to users in 're_globl' channel with re_spy::privatemsg flag
				for (auto &v : *reGlobalChannelIndex) {
					if (!(targetSession->second == v) && v->valid) { // Make sure not to send twice if client is also the recipient, and that target is valid
						if (v->listenerMode & PrivateMessage) {
							((v->webSocket))->send(message, length, code);
						}
					}
				}
			}
			break;
		}
	}

	return true;
}


// returns true on success, returns false if onMessage should terminate
bool HandleTextMessages(Session* client, uWS::WebSocket<uWS::SERVER> *ws, char* message, size_t length) {
	uWS::OpCode code = uWS::OpCode::TEXT;

	// Disconnect on invalid size
	if (!MessageSizeValid(client, ws, length, uWS::OpCode::TEXT))
		return false;

	// Read and decode who the message is being sent to
	unsigned char userIDbytes[8];
	uint64_t *targetUserID = (uint64_t*)(&userIDbytes[0]);
	dec64(message, 12, userIDbytes);

	switch (*targetUserID) {

		case RE_BROADCAST_TARGET: {
			enc64((const char*)&(client->userId), 8, message);

			// SPECIAL re_globl broadcast-message is sent to entire relay
			if (client->channelIndex == reGlobalChannelIndex) {
				for (auto &v : SessionExists) {
					if (!(ws == ((v->webSocket))) && v->valid) {
						((v->webSocket))->send(message, length, code);
					}
				}
			}
			else {
				// Send to just the channel
				for (auto &v : *client->channelIndex) {
					if (!(ws == ((v->webSocket))) && v->valid) {
						((v->webSocket))->send(message, length, code);
					}
				}

				// Send to users in 're_globl' channel with ChannelMessage flag in listenerMode
				for (auto &v : *reGlobalChannelIndex) {
					if (!(ws == ((v->webSocket))) && v->valid) {
						if (v->listenerMode & ChannelMessage) {
							((v->webSocket))->send(message, length, code);
						}
					}
				}
			}
			break;
		}

		default: {
			auto targetSession = UserIDSessionMap.find((uint64_t)*targetUserID);
			if (targetSession != UserIDSessionMap.end()) {
				enc64((const char*)&(client->userId), 8, message);

				// Send to the private message target
				if ((targetSession->second)->valid) {
					(((targetSession->second)->webSocket))->send(message, length, code);
				}

				// Send to users in 're_globl' channel with re_spy::privatemsg flag
				for (auto &v : *reGlobalChannelIndex) {
					if (!(targetSession->second == v) && v->valid) {
						if (v->listenerMode & PrivateMessage) {
							((v->webSocket))->send(message, length, code);
						}
					}
				}
			}
			break;
		}

	}
	return true;
}


/////////////////////
// MAIN FUNCTION
/////////////////
int main(int argc, char* argv[])
{
	//
	// Create special relay channel: re_globl 
	//
	{
		auto globalAdd = ChannelClientTable.insert(std::make_pair("re_globl", tbb::concurrent_unordered_set<Session*>()));
		reGlobalChannelIndex = &(globalAdd.first->second);
	}

	std::vector<std::thread *> threads(std::thread::hardware_concurrency());
	
	for (auto &thread : threads) {
		thread = new std::thread([] {
			uWS::Hub h;

			h.onMessage([](uWS::WebSocket<uWS::SERVER> *ws, char *message, size_t length, uWS::OpCode code) {
				Session* client = (Session*)ws->getUserData();

				// Wait for garbage collection and get lock
				AcquireGarbageLock gcLock = AcquireGarbageLock();

				// client is not NULL, meaning this socket has a Session
				if (client) {
					switch (code) {
					case uWS::OpCode::BINARY: {
						if (!HandleBinaryMessages(client, ws, message, length))
							return;
						break;
					}
					case uWS::OpCode::TEXT: {
						if (!HandleTextMessages(client, ws, message, length))
							return;
						break;
					}
					default: {
						DisconnectClient(client, ws, CLOSE_UNSUPPORTED, MSG_TYPE_UNSUPPORTED, sizeof(MSG_TYPE_UNSUPPORTED));
						return;
						break;
					}
					}
				}
				// FIRST PACKET: client is NULL, meaning this socket needs a Session
				else {
					if (length > 16) {
						// Disconnect user if channel name is over 16 characters
						DisconnectClient(NULL, ws, CLOSE_PROTOCOL_ERROR, MSG_CHANNEL_LENGTH_EXCEEDED, sizeof(MSG_CHANNEL_LENGTH_EXCEEDED));
						return;
					}
					std::string channelName;
					channelName.assign(message, length);

					client = new Session(ws);
					ws->send((const char*)&(client->userId), sizeof(client->userId), uWS::OpCode::BINARY);

					auto node = ChannelClientTable.find(channelName);
					if (node == ChannelClientTable.end()) {
						// Create channel if it doesnt exist
						auto insert = ChannelClientTable.insert(std::make_pair(channelName, tbb::concurrent_unordered_set<Session*>()));
						if (insert.second == true) {
							node = insert.first;
						}
					}
					// Save channel value references
					client->channelName = &node->first;
					client->channelIndex = &node->second;
					client->channelIndex->insert(client); // Add user to channel index
					SessionExists.insert(client);         // Add user to global session list
				}
			});

			h.onDisconnection([](uWS::WebSocket<uWS::SERVER>* ws, int code, char *message, size_t length) {
				// Wait for garbage collection and get lock
				AcquireGarbageLock gcLock = AcquireGarbageLock();

				Session* client = (Session*)ws->getUserData();
				if (client) {
					uint64_t dcMsgBuf[2];
					dcMsgBuf[0] = RE_BROADCAST_TARGET; // Disconnection events come from the UserID: RE_BROADCAST_TARGET
					dcMsgBuf[1] = (uint64_t)(client->userId);

					if (client->valid) {
						// If client is still valid: invalidate session
						client->valid = false;
						GarbageQueue.push(client);
					}

					// Send disconnect event to clients in the same channel
					for (auto &v : *client->channelIndex) {
						if (!(ws == (v->webSocket)) && v->valid) {
							(v->webSocket)->send((const char*)(&dcMsgBuf[0]), 16, uWS::OpCode::BINARY);
						}
					}

					// Send to disconnect events to users in 're_globl' channel with DisconnectMessage flag set in listenerMode 
					for (auto &v : *reGlobalChannelIndex) {
						if (!(ws == (v->webSocket)) && v->valid) {
							if (v->listenerMode & DisconnectMessage) {
								(v->webSocket)->send((const char*)(&dcMsgBuf[0]), 16, uWS::OpCode::BINARY);
							}
						}
					}
				}

			});

			h.onError([](void *user) {
				// Wait for garbage collection and get lock
				AcquireGarbageLock gcLock = AcquireGarbageLock();

				Session* client = (Session*)user;
				if (client && client->valid) {
					client->valid = false;
					GarbageQueue.push(client);
				}
			});

			auto TlsContext = uS::TLS::createContext(SERVER_TLS_CERTIFICATE, SERVER_TLS_PRIVATEKEY, SERVER_TLS_KEYPASSWORD);
			if (!h.listen(SERVER_PORT, TlsContext, uS::ListenOptions::REUSE_PORT)) {
				printf("Failed to listen on port %i!\n", SERVER_PORT);
			}

			//h.getDefaultGroup<uWS::SERVER>().startAutoPing(15000); // 15sec WebSocket Ping
			h.run();
		});

		// Stagger thread creation to avoid OpenSSL crash:
		// Crash occurs if creating more threads than physical cores
		std::this_thread::sleep_for(std::chrono::milliseconds(100));
	}


	std::thread gc([] {
		std::chrono::seconds THIRTY_SECONDS = std::chrono::seconds(30);
		for (;;) {
			std::this_thread::sleep_for(THIRTY_SECONDS);
			FreeGarbage();
		}
	});

	for (auto &thread : threads) {
		thread->join();
	}

	return 0;
}
