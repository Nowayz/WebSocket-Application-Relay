#include <stdint.h>
#include <vector>
#include <thread>
#include <chrono>
#include <uWS.h>
#include <ctime>
#include <atomic>
#include <immintrin.h>
#include "tbb/tbb.h"
#include "tbb/concurrent_unordered_map.h"
#include "tbb/concurrent_queue.h"
#include "tbb/concurrent_unordered_set.h"

// Miscellaneous Macros
#define WAIT_FOR_GC() while (gc_State==-1) {continue;};

// Websocket Exit Codes
#define CLOSE_PROTOCOL_ERROR  1002
#define CLOSE_UNSUPPORTED     1003
#define CLOSE_TRY_AGAIN_LATER 1013
#define CLOSE_USERID_TAKEN    4001

// Special Relay UserID Targets
#define RE_BROADCAST_TARGET 0xFFFFFFFFFFFFFFFF  // Use this target to broadcast to everybody in the channel at once
#define RE_RELAY_TARGET     0x0000000000000000

#define AUTH_TRUSTED 1


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
uint32_t THREADS;
uWS::Hub *ThreadedServer[64]; // Hard-limit 64 slots

// Signaler for the GC_State, for scheduling garbage collection
// May need to replace this with more efficient signaling, will investigate later
std::atomic<char> gc_State;


// O(n) THREAD-SAFE LOOKUP TABLES (NOTE: Future optimization can be made here) (SSE4/AVX2 HASH & Lookup)
tbb::concurrent_unordered_map<uint64_t, Session*>									UserIDSessionMap;    //	 Relay UserID  :  Session Pointer               (This was added to avoid using memory addresses as UserIDs) (Now UserID can be anything)
tbb::concurrent_unordered_set<Session*>                                             SessionExists;       //       Session  :  Is Session Pointer Valid?     (Used to confirm Point-To-Point message recepient validity)
tbb::concurrent_unordered_map<std::string, tbb::concurrent_unordered_set<Session*>> ChannelClientTable;  //  Channel Name  :  List of Subscribed Clients
tbb::concurrent_unordered_set<Session*>* reGlobalChannelIndex;

// GARBAGE COLLECTION QUEUE
tbb::concurrent_queue<Session*> GarbageQueue;



/////////////////////
// Auxiliary Functions
/////////////////
uint64_t rand64p() {
	static unsigned long long a, b;
	static int init = 1;
	if (init--) {
		_rdrand64_step(&a);
		_rdrand64_step(&b);
	}
	unsigned long long x = a; a = b;
	return a + (b = (x ^= x << 23) ^ b ^ (x >> 17) ^ (b >> 26));
}

// enc64 allocates and returns base64 decoded string, and sets output 'len' parameter
int enc64(const char* input, int len, char* output) {
	static const char b64[] =
		"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
	int i;

	if (len == 0) { // Empty input, return empty string
		return 0;
	}
	char q, *p = (char*)output;
	if (len == 1) { // Handle case of single character (prevent 1-char crash)
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

// dec64 allocates and returns base64 decoded string, and sets output 'len' parameter
int dec64(const char* src, int len, unsigned char* out) {
	static const char b64[] =
		"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

	if (len == 0) { // Empty input, return empty string
		len = 0;
		return 0;
	}
	unsigned char dtable[256], *pos, in[4], block[4], tmp;
	size_t count, olen;
	memset(dtable, 0x80, 256);
	for (unsigned char i = 0; i < sizeof(b64); i++)
		dtable[b64[i]] = i;
	dtable['='] = 0;
	count = 0;
	for (unsigned int i = 0; i < len; i++)
		if (dtable[src[i]] != 0x80)
			count++;
	olen = count / 4 * 3;
	pos = out;
	if (out == NULL) {
		len = 0;
		return 0;
	}
	count = 0;
	for (unsigned int i = 0; i < len; i++) {
		tmp = dtable[src[i]];
		if (tmp == 0x80)
			continue;
		in[count] = src[i];
		block[count] = tmp;
		count++;
		if (count == 4) {
			*pos++ = (block[0] << 2) | (block[1] >> 4);
			*pos++ = (block[1] << 4) | (block[2] >> 2);
			*pos++ = (block[2] << 6) | block[3];
			count = 0;
		}
	}
	if (pos > out) {
		if (in[2] == '=')
			pos -= 2;
		else if (in[3] == '=')
			pos--;
	}
	return pos - out;
}



/////////////////////
// Structures
/////////////////
/* Relay Session Information
	> The goal is to be as lightweight as possible, only holding information
	  needed for administration, security, or by 'Host Clients' on the relay.

	  Host Clients are special relay clients which have unique jobs
	  such as storing user information or negotiating matchmaking.

	  There is not a seperate protocol for a Host Client, and they may fill 
	  various tasks depending on the needs of an application using the relay.
*/

// Bit-flags for listening to different messages
enum re_spy { 
	none          = 0b0000,
	channelmsg    = 0b0001,
	privatemsg    = 0b0010,
	disconnectmsg = 0b0100
};
struct re_auth {
	const char* password;
	int   authLevel;
};

re_auth Auths[] = {
	{ "mickymouse", AUTH_TRUSTED }
};

struct Session {
	uWS::WebSocket<uWS::SERVER> webSocket;
	std::time_t timeOfConnection;  // When connection occured (GMT+0)
	uint64_t userId;

	const std::string* channelName;                        // Name of the Channel the user is in
	tbb::concurrent_unordered_set<Session*>* channelIndex; // Pointer to channel array for user's channel
	std::atomic<bool> valid;                               // Is socket still valid (1 if ready, 0 if disconnected and pending deletion)
	int listenerMode;
	int authLevel;   // Level 1 = Relay Query & Listener Authentication

	Session(uWS::WebSocket<uWS::SERVER> ws) {
		//printf("creating session for ws:%llX\n", ws);
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
		ws.setUserData(this);
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
			std::string tmpName = *this->channelName; 
			ChannelClientTable.unsafe_erase(tmpName);
		}

		// Erase session from global session list
		SessionExists.unsafe_erase(this);
	}
};

void FreeGarbage() {
	// Busy wait until we can cleanup safely (and hope this doesn't take forever)
	while (gc_State>0) { continue; }
	gc_State = -1;

	Session* client;
	while (GarbageQueue.try_pop(client)) {
		//printf("Free Session: 0x%016llX, WS: 0x%016llX\n", client, (client->webSocket));
		delete client;
	}
	
	//printf("SessionExists: %i\n", SessionExists.size());
	//printf("ChannelClientTable: %i\n", ChannelClientTable.size());
	//printf("UserIDSessionMap: %i\n", UserIDSessionMap.size());

	gc_State = 0;
}



/////////////////////
// Entry Point
/////////////////
int main(int argc, char* argv[])
{
	// Initialize some runtime globals
	THREADS = std::thread::hardware_concurrency(); // Default Configation: Use all cores
	THREADS = THREADS ? THREADS : 4;               // Default to 4 threads if failed to detect

	// Create special relay channel: re_globl 
	{
		auto globalAdd = ChannelClientTable.insert(std::make_pair("re_globl", tbb::concurrent_unordered_set<Session*>()));
		if (globalAdd.second) {
			reGlobalChannelIndex = &(globalAdd.first->second);
		}
		else {
			//printf("wtf just happend \n");
			return -1;
		}
	}

	try {
		uWS::Hub h;

		h.onConnection([](uWS::WebSocket<uWS::SERVER> ws, uWS::HTTPRequest ui) {
			// Get random thread with hardcore rdrand
			unsigned long long n; 
			_rdrand64_step(&n);
			n %= THREADS;

			//printf("Client connected (WS: %llX); transfered to thread %i\n", ws, n);

			ws.setUserData(NULL);
			ws.transfer(&ThreadedServer[n]->getDefaultGroup<uWS::SERVER>());
		});

		// launch the threads with their servers
		for (int i = 0; i < THREADS; i++) {
			new std::thread([i] {
				// Create worker on each thread & register event handlers
				ThreadedServer[i] = new uWS::Hub();

				ThreadedServer[i]->onDisconnection([&i](uWS::WebSocket<uWS::SERVER> ws, int code, char *message, size_t length) {
					Session* client = (Session*)ws.getUserData();
					uint64_t dcMsgBuf[2];
					dcMsgBuf[0] = RE_BROADCAST_TARGET;
					dcMsgBuf[1] = (uint64_t)(client->userId);

					// Busy wait until garbage collection has finished
					WAIT_FOR_GC();
					gc_State++;

					if (client && client->valid) {
						// Invalidate session
						client->valid = false;
						GarbageQueue.push(client);
						for (auto &v : *client->channelIndex) {
							if (!(ws == ((v->webSocket))) && v->valid) {
								((v->webSocket)).send((const char*)(&dcMsgBuf[0]), 16, uWS::OpCode::BINARY);
							}
						}
						// Send to disconnect events to users in 're_globl' channel with re_spy::disconnectmsg flag
						for (auto &v : *reGlobalChannelIndex) {
							if (!(ws == ((v->webSocket))) && v->valid) {
								if (v->listenerMode & re_spy::disconnectmsg) {
									((v->webSocket)).send((const char*)(&dcMsgBuf[0]), 16, uWS::OpCode::BINARY);
								}
							}
						}
					}

					gc_State--;
				});

				ThreadedServer[i]->onMessage([&i](uWS::WebSocket<uWS::SERVER> ws, char *message, size_t length, uWS::OpCode code) {
					Session* client = (Session*)ws.getUserData();

					// Busy wait until garbage collection has finished
					WAIT_FOR_GC();
					gc_State++;

					if (client) {
						if (code == uWS::OpCode::BINARY) {
							// Disconnect user if message too small to be valid
							if (length <= 8) {
								client->valid = false;
								GarbageQueue.push(client);
								ws.close(CLOSE_PROTOCOL_ERROR, "Protocol Violation", 19);
								gc_State--;
								return;
							}

							// Read message target and send
							uint64_t *targetUserID = (uint64_t*)(&message[0]);
							if (*targetUserID == RE_BROADCAST_TARGET) {
								// Overwrite message with sender's UserID
								*targetUserID = client->userId;

								// SPECIAL re_globl broadcast-message is sent to entire relay
								if (client->channelIndex == reGlobalChannelIndex) {
									for (auto &v : SessionExists) {
										if (!(ws == ((v->webSocket))) && v->valid) {
											((v->webSocket)).send(message, length, code);
										}
									}
								}
								else {
									// Send to just the channel
									for (auto &v : *client->channelIndex) {
										if ( !(ws == ((v->webSocket))) && v->valid) {
											((v->webSocket)).send(message, length, code);
										}
									}

									// Send to users in 're_globl' channel with re_spy::channelmsg flag
									for (auto &v : *reGlobalChannelIndex) {
										if (!(ws == ((v->webSocket))) && v->valid) {
											if (v->listenerMode & re_spy::channelmsg) { // Check global Relay Channel listening bit
												((v->webSocket)).send(message, length, code);
											}
										}
									}
								}
							}
							else if (*targetUserID == RE_RELAY_TARGET) {
								char msgBuffer[24];
								size_t msgLen;
								if (length < 9) { gc_State--; return; }
								switch (message[8]) {
								case 0:
									msgLen = length - 9;
									if ((msgLen > 0) && (msgLen < 24)) {
										strncpy(msgBuffer, &message[9], msgLen);
										re_auth* authNode = NULL;
										for (auto &auth : Auths) {
											if (!strncmp(msgBuffer, auth.password, msgLen)) {
												authNode = &auth;
											}
										}
										if (authNode) {
											//printf("Client[%016llX] authenticated with \"%s\"; setting authLevel to %i\n", client, authNode->password, authNode->authLevel);
											client->authLevel = authNode->authLevel;
										}
									}
									break;
								case 1:
									if (length != 10) { gc_State--; return; }
									if (client->authLevel == 1) {
										//printf("Client[%016llX] listener mode to %i\n", client, message[9]);
										client->listenerMode = message[9];
									}
									break;
								case 2:
									if (length != 9) { gc_State--; return; }
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
										ws.send(buffer, payloadSize, uWS::OpCode::BINARY);
										free(buffer);
									}
									break;
								case 3:
									// If a Session is authenticated, allow assignment of any untaken userId
									msgLen = length - 9;
									if (client->authLevel < AUTH_TRUSTED) { gc_State--; return; }
									if (msgLen != 8) { gc_State--; return; }
									if (UserIDSessionMap.count(*(uint64_t*)(&message[9]))) {
										Session* tmpclient = UserIDSessionMap[*(uint64_t*)(&message[9])];
										tmpclient->userId = NULL;
										if (tmpclient->valid) {
											tmpclient->valid = false;
											((tmpclient->webSocket)).close(CLOSE_USERID_TAKEN, "UserId Taken", 13);
											GarbageQueue.push(tmpclient);
										}
									}
									UserIDSessionMap[*(uint64_t*)(&message[9])] = client;
									UserIDSessionMap.unsafe_erase(client->userId);
									client->userId = *(uint64_t*)(&message[9]);
									break;
								default:
									client->valid = false;
									GarbageQueue.push(client);
									ws.close(CLOSE_PROTOCOL_ERROR, "Protocol Violation", 19);
									break;
								}
							}
							else {
								auto targetSession = UserIDSessionMap.find((uint64_t)*(size_t*)message); // Since message[0] has the target we just dereference with size_t
								if (targetSession != UserIDSessionMap.end()) {
									*targetUserID = client->userId; // Prefix message with sender's UserID

									// Send Private Message to Target
									if ((targetSession->second)->valid) {
										(((targetSession->second)->webSocket)).send(message, length, code);
									}

									// Send Private Message to users in 're_globl' channel with re_spy::privatemsg flag
									for (auto &v : *reGlobalChannelIndex) {
										if ( !(targetSession->second == v) && v->valid ) { // Make sure not to send twice if client is also the recipient, and that target is valid
											if (v->listenerMode & re_spy::privatemsg) {
												((v->webSocket)).send(message, length, code);
											}
										}
									}
								}
							}
						}
						else if (code == uWS::OpCode::TEXT) {
							// Disconnect user if message too small to be valid
							if (length <= 12) {
								client->valid = false;
								GarbageQueue.push(client);
								ws.close(CLOSE_PROTOCOL_ERROR, "Protocol Violation", 19);
								gc_State--;
								return;
							}

							// Read and decode who the message is being sent to
							unsigned char userIDbytes[8];
							uint64_t *targetUserID = (uint64_t*)(&userIDbytes[0]);
							dec64(message, 12, userIDbytes);

							if (*targetUserID == RE_BROADCAST_TARGET) {
								enc64((const char*)&(client->userId), 8, message);

								// SPECIAL re_globl broadcast-message is sent to entire relay
								if (client->channelIndex == reGlobalChannelIndex) {
									for (auto &v : SessionExists) {
										if (!(ws == ((v->webSocket))) && v->valid) {
											((v->webSocket)).send(message, length, code);
										}
									}
								}
								else {
									// Send to just the channel
									for (auto &v : *client->channelIndex) {
										if (!(ws == ((v->webSocket))) && v->valid) {
											((v->webSocket)).send(message, length, code);
										}
									}

									// Send to users in 're_globl' channel with re_spy::channelmsg flag
									for (auto &v : *reGlobalChannelIndex) {
										if (!(ws == ((v->webSocket))) && v->valid) {
											if (v->listenerMode & re_spy::channelmsg) {
												((v->webSocket)).send(message, length, code);
											}
										}
									}
								}
							}
							else {
								auto targetSession = UserIDSessionMap.find((uint64_t)*targetUserID);
								if (targetSession != UserIDSessionMap.end()) {
									enc64((const char*)&(client->userId), 8, message);

									// Send to the private message target
									if ((targetSession->second)->valid) {
										(((targetSession->second)->webSocket)).send(message, length, code);
									}

									// Send to users in 're_globl' channel with re_spy::privatemsg flag
									for (auto &v : *reGlobalChannelIndex) {
										if (!(targetSession->second == v) && v->valid) {
											if (v->listenerMode & re_spy::privatemsg) {
												((v->webSocket)).send(message, length, code);
											}
										}
									}
								}
							}
						}
						else { // Disconnect client if a recieved type is neither Text or Binary
							client->valid = false;
							GarbageQueue.push(client);
							ws.close(CLOSE_UNSUPPORTED, "Type Unsupported", 17);
							gc_State--;
							return;
						}
						
					}
					// FIRST PACKET: Join Channel
					else {
						// Disconnect user for channel name over 16 characters
						if (length > 16) {
							ws.close(CLOSE_PROTOCOL_ERROR, "Channel length exceeded (16 bytes)", 35);
						}
						else {
							std::string channelName;
							channelName.assign(message, length);

							client = new Session(ws);
							ws.send((const char*)&(client->userId), sizeof(client->userId), uWS::OpCode::BINARY);

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
					}
					gc_State--;
 				});

				ThreadedServer[i]->onError([&i](void *user) {
					WAIT_FOR_GC();
					gc_State++;

					Session* client = (Session*)user;
					if (client && client->valid) {
						client->valid = false;
						GarbageQueue.push(client);
					}
					gc_State--;
				});

				ThreadedServer[i]->getDefaultGroup<uWS::SERVER>().startAutoPing(15000);
				ThreadedServer[i]->getDefaultGroup<uWS::SERVER>().addAsync();
				ThreadedServer[i]->run();
			});

		}

		std::thread gc([]{
			std::chrono::seconds THIRTY_SECONDS = std::chrono::seconds(30);
			for (;;) {
				std::this_thread::sleep_for(THIRTY_SECONDS);
				FreeGarbage();
			}
		});

		h.getDefaultGroup<uWS::SERVER>().addAsync();
		h.listen(80);
		h.run();
	}
	catch (...) {
		//printf("ERR_LISTEN\n");
	}

	return 0;
}
