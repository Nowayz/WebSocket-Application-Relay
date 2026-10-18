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
	
	USERS : {},

	// UserId Conversion
	UInt8UserIdToBase64 : userid=>btoa(String.fromCharCode.apply(null, userid)),
	Base64ToUInt8UserID : userid=>new Uint8Array(atob(userid).map(x=>x.charCodeAt())),

	// Miscellaneous
	ArrayToStr : x=>String.fromCharCode.apply(null, x),                     // Integer array to string
	StrToArray : x=>Uint8Array.from(Array.from(x).map(x=>x.charCodeAt())),  // String to Uint8Array
	UserIdHex: x=>'0x'+Array.from(x).map(x=>x.toString(16).padStart(2,'0')).reverse().join('')  // Convert UserId to Hex String
}

////////////////
// Auxillary Routines
//////////
String.prototype.map = Array.prototype.map;

function ArrayEq(a,b) {
	for(let i = a.length-1; i; i--)
		if(a[i]!==b[i]) return false;
	return true;
}

function UInt8UserIdToBase64(userid) {
	return btoa(String.fromCharCode.apply(null, userid));
}

function Base64ToUInt8UserID(userid) {
	return new Uint8Array(atob(userid).map(function(v){return v.charCodeAt();}));
}


////////////////
// Relay Class Prototype
//////////
class Relay {
	constructor (relayURL, channelName) {
		// Setup Methods
		this.FirstMessageHandler = this.FirstMessageHandler.bind(this);
		this.MessageDispatcher   = this.MessageDispatcher.bind(this);

		// Setup WebSocket
		this.ws = new WebSocket(relayURL);
		this.ws.binaryType = "arraybuffer";
		this.ws.addEventListener('message', this.FirstMessageHandler);
		this.ws.onopen = this.JoinChannel.bind(this);
		
		// Callbacks
		this.onClientDisconnect    = null; // Callback for client disconnects: onClientDisconnect(userId)
		this.onSubscription        = null; // Callback for initial channel join: onSubscription(this)
		this.BinaryMessageHandlers = {};
		this.TextMessageHandlers   = {};
		this.VariableCallbacks     = []; // List of callback for variable requests

		// Setup Properties
		this.channelName = channelName;
		this.ready       = false; // ready set when channel is joined (after first message handler)
		this.userId      = null;  // Assigned in FirstMessageHandler

		this.SetMessageHandler(re.BINARY, 200, (userId, msg)=>{
            let msgValue = new Uint8Array(msg);
			let callback = this.VariableCallbacks.shift();
			callback(msgValue);
        });
	}

	JoinChannel(e) {
		// After connecting, specify channel to join
		this.ws.send(this.channelName);
	}

	// SetMessageHandler(msgType, OpCode, callback)
	//    msgType : re.TEXT or re.BINARY
	//     OpCode : Operation Code number between 0-255 ( As specified in re.OP )
	//   callback : callback(sender, msg)
	//                sender : userId of message sender
	//                   msg : Message payload ( Uint8Array for re.BINARY |OR| Object for re.TEXT)
	SetMessageHandler(msgType, OpCode, callback) {
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

	MessageDispatcher(e) {
		if (typeof e.data === "string") {
			let obj    = JSON.parse(e.data.substring(12));
			let sender = e.data.substring(0,12);
			let op     = obj.op;
			let handler = this.TextMessageHandlers[op];
			if (handler) {
				handler(sender, obj);
			}
		} 
		else {
			let msg    = new Uint8Array(e.data); // ?-Byte Payload
			let sender = msg.subarray(0,8);      // 8-Byte Sender UserID
			if (ArrayEq(re.BINARY_BROADCAST, sender)) {
				let handler = this.onClientDisconnect;
				if (handler) {
					handler(msg.subarray(8));
				}
			} 
			else {
				let op = msg[8]; // 1-Byte OpCode
				// Dispatch to OpCode handler
				let handler = this.BinaryMessageHandlers[op];
				if (handler) {
					msg = msg.buffer.slice(9);
					handler(sender, msg);
					//handler(sender, msg.subarray(9));
				}
			}
	
		}
	}

	FirstMessageHandler(e) {
		// Setup userId
		let U8_MSG = new Uint8Array(e.data);
		this.userId = U8_MSG;
	
		// Dispatch callback on channel subscription
		if (this.onSubscription) { // This should pretty much always be defined
			this.onSubscription.apply(this);
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
	bSendTo(target, OpCode, msg) {
		if(msg instanceof ArrayBuffer) {
			msg = new Uint8Array(msg);
		}
		let msgLength;
		if (typeof msg === 'undefined') {
			msgLength = 0;
		} else {
			msgLength = msg.byteLength;
		}
		let payLoad = new Uint8Array(9+msgLength);
		if (typeof target === 'string') {
			payLoad.set(Base64ToUInt8UserID(target));
		} else {
			payLoad.set(target);
		}
		payLoad[8] = OpCode;
		if(msgLength > 0) {
			payLoad.set(msg, 9);
		}
		//console.log(payLoad);
		this.ws.send(payLoad);
	}

	// Relay.tSendTo(target, OpCode, msg)
	//  * Sends packet to relay in JSON format
	//      target : 8-byte target as Uint8Array |OR| 12-byte base64 string
	//      obj    : Message to transmit: as Javascript object (Which also contains 'op' field specifying the opcdoe)
	tSendTo(target, obj) {
		let payLoad;
		if (typeof target === 'string') {
			payLoad = target;
		} else {
			payLoad = UInt8UserIdToBase64(target);
		}
		//console.log(payLoad+=JSON.stringify(obj));
		this.ws.send(payLoad+=JSON.stringify(obj));
	}

	SetChannelVar(key, value) {
		let keyarr   = Array.from(key).map(x=>x.charCodeAt());
		let valuearr;
		if(value instanceof Uint8Array)
			valuearr = Array.from(value);
		else
			valuearr = Array.from(value).map(x=>x.charCodeAt());

		let msg = [keyarr.length].concat(keyarr).concat(valuearr);
		msg = Uint8Array.from(msg);
		this.bSendTo(re.RELAY_QUERY, 4, msg);
	}

	GetChannelVar(key, callback) {
		let keyarr   = Array.from(key).map(x=>x.charCodeAt());
		let msg = Uint8Array.from(keyarr);
		this.VariableCallbacks.push(callback);
		this.bSendTo(re.RELAY_QUERY, 5, msg);
	}
}


