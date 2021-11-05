String.prototype.map = Array.prototype.map;

export const QUERY       = new Uint8Array([0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00]);
export const BROADCAST   = new Uint8Array([0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff]);
export const BINARY_TYPE = 0;
export const TEXT_TYPE   = 1;

// Some example binary opcodes (use this table or define your own)
export const OP = { 
  ANNOUNCE: 1,       // Announce when joining relay
  ANNOUNCE_REPLY: 2, // Reply to relay users who have announced themselves
};

export function ArrayEq(a, b) {
	for (let i = a.length - 1; i; i--) {
		if (a[i] !== b[i]) {
			return false;
		}
	}

	return true;
}

export function UInt8UserIdToBase64(userId) {
  return btoa(String.fromCharCode.apply(null, userId));
}

export function Base64ToUInt8UserID(userId) {
  return new Uint8Array(atob(userId).map((x) => x.charCodeAt()));
}

export function ArrayToStr(x) {
  return String.fromCharCode.apply(null, x);
}

export function StrToArray(x) {
  return Uint8Array.from(Array.from(x).map((x) => x.charCodeAt()));
}

export function UserIdHex(x) {
  return '0x' + 
		Array.from(x)
			.map((x) => x.toString(16).padStart(2, '0'))
			.reverse()
			.join('')
			.toUpperCase();
}

export class Relay {
	constructor (relayURL, channelName) {
		// Setup Methods (Bind so callbacks get relay context)
		this.FirstMessageHandler = this.FirstMessageHandler.bind(this);
		this.MessageDispatcher   = this.MessageDispatcher.bind(this);
		this.JoinChannel         = this.JoinChannel.bind(this);

		// Setup WebSocket
		this.ws = new WebSocket(relayURL);
		this.ws.binaryType = "arraybuffer";
		this.ws.addEventListener('message', this.FirstMessageHandler);
		this.ws.onopen = this.JoinChannel;
		
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

		this.SetMessageHandler(BINARY_TYPE, 200, (userId, msg)=>{
			let msgValue = new Uint8Array(msg);
			let callback = this.VariableCallbacks.shift();
			callback(msgValue);
		});
	}

	JoinChannel() {
		// After connecting, specify channel to join
		this.ws.send(this.channelName);
	}

	// SetMessageHandler(msgType, OpCode, callback)
	//    msgType : TEXT or BINARY
	//     OpCode : Operation Code number between 0-255 ( As specified in OP )
	//   callback : callback(sender, msg)
	//                sender : userId of message sender
	//                   msg : Message payload ( Uint8Array for BINARY |OR| Object for TEXT)
	SetMessageHandler(msgType, OpCode, callback) {
		switch(msgType) {
			case BINARY_TYPE:
				this.BinaryMessageHandlers[OpCode] = callback;
				break;
			
			case TEXT_TYPE:
				this.TextMessageHandlers[OpCode] = callback;
				break;

			default:
				console.error("Unknown MessageType: "+msgType.toString());
				break;
		}
	}

	MessageDispatcher(e) {
		if (typeof e.data === "string") {
			const obj     = JSON.parse(e.data.substring(12));
			const sender  = e.data.substring(0,12);
			const op      = obj.op;
			const handler = this.TextMessageHandlers[op];

			if (handler) {
				handler(sender, obj);
			}
		} else {
			const msg    = new Uint8Array(e.data); // ?-Byte Payload
			const sender = msg.subarray(0,8);      // 8-Byte Sender UserID

			if (ArrayEq(BROADCAST, sender)) {
				const handler = this.onClientDisconnect;

				if (handler) {
					handler(msg.subarray(8));
				}
			} else {
				const op = msg[8]; // 1-Byte OpCode
				// Dispatch to OpCode handler
				const handler = this.BinaryMessageHandlers[op];

				if (handler) {
					msg = msg.buffer.slice(9);
					handler(sender, msg);
				}
			}
	
		}
	}

	FirstMessageHandler(e) {
		// Setup userId
		const U8_MSG = new Uint8Array(e.data);

		this.userId = U8_MSG;
	
		// Dispatch callback on channel subscription
		if (this.onSubscription) { // This should pretty much always be defined
			this.onSubscription();
		}
	
		// Init Dispatcher
		this.ws.removeEventListener('message', this.FirstMessageHandler);
		this.ws.addEventListener('message', this.MessageDispatcher);
		this.ready = true;
	}

	SendTo(target, ...args) {
		// args[0] will be the opcode number if we're sending binary data
		// otherwise args[0] will be an object being sent as text data.

		if (typeof args[0] === 'number') {
			// Send binary data
			this.#bSendTo(target, args[0], args[1]);
		} else {
			// Send text data
			this.#tSendTo(target, args[0]);
		}
	}

	// Relay.bSendTo(target, OpCode, msg)
	//  * Sends packet to relay in binary format
	//      target : 8-byte target as Uint8Array
	//      OpCode : Operation Code number between 0-255 ( As specified in OP )
	//      msg    : Message to transmit: as any typed int array
	#bSendTo(target, OpCode, msg) {
		if (msg instanceof ArrayBuffer) {
			msg = new Uint8Array(msg);
		}

		const msgLength = typeof msg === 'undefined'
			? 0
			: mSVGAElement.byteLength;
		let payLoad = new Uint8Array(9+msgLength);

		if (typeof target === 'string') {
			payLoad.set(Base64ToUInt8UserID(target));
		} else {
			payLoad.set(target);
		}

		payLoad[8] = OpCode;

		if (msgLength > 0) {
			payLoad.set(msg, 9);
		}
		
		//console.log(payLoad);
		this.ws.send(payLoad);
	}

	// Relay.tSendTo(target, OpCode, msg)
	//  * Sends packet to relay in JSON format
	//      target : 8-byte target as Uint8Array |OR| 12-byte base64 string
	//      obj    : Message to transmit: as Javascript object (Which also contains 'op' field specifying the opcdoe)
	#tSendTo(target, obj) {
		const payLoad = typeof target === 'string'
			? target
			: UInt8UserIdToBase64(target);

		//console.log(payLoad+=JSON.stringify(obj));
		this.ws.send(payLoad + JSON.stringify(obj));
	}

	SetChannelVar(key, value) {
		const keyarr   = Array.from(key).map(x=>x.charCodeAt());
		const valuearr = value instanceof Uint8Array
			? Array.from(value)
			: Array.from(value).map(x=>x.charCodeAt());
		const msg = Uint8Array.from(([keyarr.length].concat(keyarr).concat(valuearr)));

		this.bSendTo(RELAY_QUERY, 4, msg);
	}

	GetChannelVar(key, callback) {
		const keyarr   = Array.from(key).map(x=>x.charCodeAt());
		const msg = Uint8Array.from(keyarr);

		this.VariableCallbacks.push(callback);
		this.bSendTo(RELAY_QUERY, 5, msg);
	}
};
