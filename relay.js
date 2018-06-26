"use strict";
////////////////
// Global Relay Definitions
//////////
const re = {
	/* Object Constants */
	BINARY_BROADCAST: new Uint8Array([0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff]),
	RELAY_QUERY: new Uint8Array([0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00]),
	TEXT_BROADCAST: '//////////8=',

	/* Enum Constants */
	BINARY: 0,
	TEXT: 1,
	
	/* Protocol Definition (Some Defaults) */
	OP: {
		ANNOUNCE: 1,        // Send upon channel connection
		ANNOUNCE_REPLY: 2,  // Private reply to an announce message
	},
	
	USERS : {}
}


////////////////
// Auxillary Routines/Definitions
//////////
String.prototype.map = Array.prototype.map;

function printUserID(buf)
{
	var uid = '';
	for(var i = 7; i>=0; i--)
	{
		uid+=("00"+buf[i].toString(16)).slice(-2);
	}
	uid='0x'+uid;
	console.log(uid);
}

function UInt8UserIdToBase64(userid) {
	return btoa(String.fromCharCode.apply(null, userid));
}
function Base64ToUInt8UserID(userid) {
	return new Uint8Array(atob(userid).map(function(v){return v.charCodeAt();}));
}
function ArrayEq(a,b) {
	for(let i = a.length-1; i; i--)
		if(a[i]!==b[i]) return false;
	return true;
}
function UInt8Str(str) {
	let u8 = new Uint8Array(str.length);
	for(let i = str.length; i--;) {
		u8[i]=str.charCodeAt(i);
	}
	return u8;
}
function AsciiStr(u8data) {
	return String.fromCharCode.apply(null, u8data);
}

////////////////
// Relay Class Prototype
//////////
function Relay(relayURL, channelName) {
	// Setup Methods
	this.FirstMessageHandler = this.FirstMessageHandler.bind(this);
	this.MessageDispatcher   = this.MessageDispatcher.bind(this);

	// Setup WebSocket
	this.ws = new WebSocket(relayURL);
	this.ws.binaryType = "arraybuffer";
	this.ws.addEventListener('message', this.FirstMessageHandler);
	this.ws.onopen = this.JoinChannel.bind(this);
	
	// Setup Properties
	this.BinaryMessageHandlers = {};
	this.TextMessageHandlers   = {};
	this.channelName           = channelName;
	this.ready                 = false;
	this.userId                = null;  // Set in FirstMessageHandler
}

Relay.prototype.JoinChannel = function(e) {
	// After connecting, specify channel to join
	this.ws.send(this.channelName);
}

// SetMessageHandler(msgType, OpCode, callback)
//    msgType : re.TEXT or re.BINARY
//     OpCode : Operation Code number between 0-255 ( As specified in re.OP )
//   callback : callback(sender, msg)
//                sender : UserID of message sender
//                   msg : Message payload ( Uint8Array for re.BINARY |OR| Object for re.TEXT)
Relay.prototype.SetMessageHandler = function(msgType, OpCode, callback) {
	switch(msgType){
		case re.BINARY:
			this.BinaryMessageHandlers[OpCode] = callback.bind(this);
		break;
		
		case re.TEXT:
			this.TextMessageHandlers[OpCode] = callback.bind(this);
		break;

		default:
			console.error("Unknown MessageType: "+msgType.toString());
		break;
	}
}

Relay.prototype.MessageDispatcher = function(e) {
	if (typeof e.data === "string") {
		var obj    = JSON.parse(e.data.substring(12));
		var sender = e.data.substring(0,12);
		var op     = obj.op;
		var handler = this.TextMessageHandlers[op];
		if (handler) {
			handler(sender, obj);
		}
	} 
	else {
		var msg    = new Uint8Array(e.data); // ?-Byte Payload
		var sender = msg.subarray(0,8);      // 8-Byte Sender UserID
		if(ArrayEq(re.BINARY_BROADCAST, sender)) {
			let handler = this.onClientDisconnect;
			if (handler) {
				handler(msg.subarray(8));
			}
		} 
		else {
			var op = msg[8];                 // 1-Byte OpCode
			// Dispatch to OpCode handler
			let handler = this.BinaryMessageHandlers[op];
			if (handler) {
				handler(sender, msg.subarray(9));
			}
		}

	}
}

Relay.prototype.FirstMessageHandler = function(e) {
	// Setup UserID
	var U8_MSG = new Uint8Array(e.data);
	this.userId = U8_MSG;

	// Dispatch ON_CONNECT Event
	if (this.onSubscription) { // This should pretty much always be defined, throw error if its not
		this.onSubscription.call(this);
	} else {
		console.warn('Relay.onSubscription undefined! Ignore warning if intentional...')
	}

	// Init Dispatcher
	this.ws.removeEventListener('message', this.FirstMessageHandler);
	this.ws.addEventListener('message', this.MessageDispatcher);
	this.ready = true;
}

// Relay.bSendTo(target, OpCode, msg)
//  * Sends packet to relay in binary format
//      target : 8-byte target as Uint8Array
//      OpCode : Operation Code number between 0-255 ( As specified in re.OP )
//      msg    : Message to transmit: as any typed int array
Relay.prototype.bSendTo = function(target, OpCode, msg) {
	var msgLength;
	if (typeof msg === 'undefined') {
		msgLength = 0;
	} else {
		msgLength = msg.byteLength;
	}
	var payLoad = new Uint8Array(9+msgLength);
	if (typeof target === 'string') {
		payLoad.set(Base64ToUInt8UserID(target));
	} else {
		payLoad.set(target);
	}
	payLoad[8] = OpCode;
	if(msgLength > 0) {
		payLoad.set(msg, 9);
	}
	this.ws.send(payLoad);
}

// Relay.tSendTo(target, OpCode, msg)
//  * Sends packet to relay in JSON format
//      target : 8-byte target as Uint8Array |OR| 12-byte base64 string
//      obj    : Message to transmit: as Javascript object (Which also contains 'op' field specifying the opcdoe)
Relay.prototype.tSendTo = function(target, obj) {
	var payLoad;
	if (typeof target === 'string') {
		payLoad = target;
	} else {
		payLoad = UInt8UserIdToBase64(target);
	}
	this.ws.send(payLoad+=JSON.stringify(obj));
}