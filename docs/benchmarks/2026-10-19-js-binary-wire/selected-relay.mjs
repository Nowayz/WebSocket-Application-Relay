import { Encoder, Decoder } from 'cbor-x';

export const BROADCAST = 0xffffffffffffffffn;
export const RELAY = 0n;
export const LISTEN = Object.freeze({ CHANNEL: 1, PRIVATE: 2, DISCONNECT: 4 });

// Standard CBOR only: record-extension tags would bind the server to this codec.
const encoder = new Encoder({ useRecords: false, mapsAsObjects: true, tagUint8Array: false });
const decoder = new Decoder({ mapsAsObjects: true });
// cbor-x shares output state: nested command capture must not reenter it.
let encoding = 0;
function encode(value) {
  if (encoding !== 0) throw new Error('Relay serialization cannot be reentered');
  encoding++;
  try { return encoder.encode(value); }
  finally { encoding--; }
}
function decodeMessage(data) {
  // A numeric string selects Uint8Array's length overload, not a byte view.
  if (typeof data === 'string') throw new TypeError('Relay requires binary CBOR messages');
  return decoder.decode(new Uint8Array(data));
}
// WebSocket.close() permits 1000 or application codes in 3000..4999.
const CLIENT_PROTOCOL_ERROR = 4002;

function requestTimeout(value) {
  // Both supported timer APIs use a signed 32-bit millisecond delay.
  if (!(value > 0 && Number.isFinite(value) && value <= 2147483647)) {
    throw new RangeError('requestTimeoutMs must be positive, finite, and at most 2147483647');
  }
  return value;
}

function asUserId(value) {
  if (typeof value === 'number' && !Number.isSafeInteger(value)) throw new RangeError('Use BigInt for uint64 IDs');
  const id = BigInt(value);
  if (id < 0n || id > BROADCAST) throw new RangeError('Target is outside uint64');
  return id;
}

// Wire IDs must be integers, never values accepted only through BigInt coercion.
function wireId(value, session = false) {
  if (typeof value !== 'number' && typeof value !== 'bigint') throw new TypeError('Invalid session ID');
  const id = asUserId(value);
  if (session && (id === RELAY || id === BROADCAST)) throw new RangeError('Reserved session ID');
  return id;
}

function validateChannelListing(rows) {
  if (!Array.isArray(rows)) throw new Error('Invalid channel listing response');
  for (const row of rows) {
    if (!Array.isArray(row) || row.length !== 2
      || !(row[0] instanceof Uint8Array) || row[0].length > 16) {
      throw new Error('Invalid channel listing response');
    }
    const population = row[1];
    if (!((Number.isSafeInteger(population) && population >= 0)
      || (typeof population === 'bigint' && population >= 0n && population <= BROADCAST))) {
      throw new Error('Invalid channel listing response');
    }
  }
}

/** Browser/Node 22+ client. All IDs stay BigInt, including small fixed IDs. */
export class Relay extends EventTarget {
  #pending = null;
  #controls = Promise.resolve();

  constructor(ws, userId, requestTimeoutMs = 10_000) {
    super();
    this.ws = ws;
    this.userId = wireId(userId, true);
    this.requestTimeoutMs = requestTimeout(requestTimeoutMs);
    const message = event => {
      try {
        const envelope = decodeMessage(event.data);
        if (!Array.isArray(envelope) || envelope.length !== 2) throw new Error('Invalid relay envelope');
        const [rawSender, value] = envelope;
        const sender = wireId(rawSender);
        if (sender === RELAY) {
          if (!Array.isArray(value) || value.length !== 2) throw new Error('Invalid relay response');
          const [op, result] = value;
          if (!this.#pending || this.#pending.op !== op) throw new Error('Unexpected relay response');
          if (op === 2) validateChannelListing(result);
          // Apply the acknowledged identity before another frame is dispatched.
          if (op === 3) this.userId = wireId(result, true);
          if ((op === 0 || op === 4) && typeof result !== 'boolean') throw new Error('Invalid boolean response');
          if (op === 1 && !((Number.isInteger(result) && result >= 0 && result <= 7)
            || (typeof result === 'bigint' && result >= 0n && result <= 7n))) {
            throw new Error('Invalid listener response');
          }
          if (op === 200 && (!Array.isArray(result) || result.length !== 2 || typeof result[0] !== 'boolean')) {
            throw new Error('Invalid variable response');
          }
          this.#pending.resolve(result);
        } else if (sender === BROADCAST) {
          this.dispatchEvent(new CustomEvent('disconnect', { detail: { userId: wireId(value, true) } }));
        } else {
          this.dispatchEvent(new CustomEvent('message', { detail: { sender, value } }));
        }
      } catch (error) {
        this.#pending?.reject(error);
        this.dispatchEvent(new CustomEvent('error', { detail: error }));
        ws.close(CLIENT_PROTOCOL_ERROR, 'Invalid Relay Response');
      }
    };
    const closed = event => {
      ws.removeEventListener('message', message);
      ws.removeEventListener('close', closed);
      this.#pending?.reject(new Error(`Relay closed (${event.code}): ${event.reason}`));
      this.dispatchEvent(new CustomEvent('close', { detail: { code: event.code, reason: event.reason } }));
    };
    ws.addEventListener('message', message);
    ws.addEventListener('close', closed);
  }

  static async connect(url, channel, { requestTimeoutMs = 10_000, WebSocketClass = globalThis.WebSocket } = {}) {
    requestTimeout(requestTimeoutMs);
    const channelBytes = typeof channel === 'string' ? new TextEncoder().encode(channel) : channel;
    if (!(channelBytes instanceof Uint8Array) || channelBytes.length > 16) throw new RangeError('Channel must contain at most 16 bytes');
    // Capture the channel before asynchronous socket setup lets its owner reuse it.
    const subscription = encode(channel);
    const ws = new WebSocketClass(url, 'war.cbor.v1');
    ws.binaryType = 'arraybuffer';
    return new Promise((resolve, reject) => {
      const cleanup = () => {
        clearTimeout(timer);
        ws.removeEventListener('open', open);
        ws.removeEventListener('message', welcome);
        ws.removeEventListener('close', closed);
        ws.removeEventListener('error', closed);
      };
      const fail = (error, code, reason) => {
        cleanup();
        reject(error);
        ws.close(code, reason);
      };
      const closed = () => fail(new Error('Relay closed before subscription'));
      const open = () => {
        try { ws.send(subscription); } catch (error) { fail(error); }
      };
      const welcome = event => {
        cleanup();
        try {
          const id = decodeMessage(event.data);
          resolve(new Relay(ws, id, requestTimeoutMs));
        } catch (error) { fail(error, CLIENT_PROTOCOL_ERROR, 'Invalid Subscription'); }
      };
      const timer = setTimeout(() => fail(new Error('Relay join timeout')), requestTimeoutMs);
      ws.addEventListener('close', closed, { once: true });
      ws.addEventListener('error', closed, { once: true });
      ws.addEventListener('open', open, { once: true });
      ws.addEventListener('message', welcome, { once: true });
    });
  }

  get ready() { return this.ws.readyState === 1; }

  send(target, value) {
    if (!this.ready) throw new Error('Relay is closed');
    // Number inputs larger than the safe-integer range have already lost precision.
    target = asUserId(target);
    this.ws.send(encode([target, value]));
  }

  broadcast(value) { this.send(BROADCAST, value); }
  close() {
    this.#pending?.reject(new Error('Relay is closed'));
    this.ws.close(1000);
  }

  /** Serialize commands: the deliberately small wire protocol has no request IDs. */
  request(op, value) {
    const deferred = encoding;
    let packet, captured, encodingFailure;
    const result = this.#controls.then(() => new Promise((resolve, reject) => {
      const delay = requestTimeout(this.requestTimeoutMs);
      if (encodingFailure) { reject(encodingFailure.error); return; }
      if (deferred) packet = encode(captured);
      const finish = callback => value => { clearTimeout(timer); this.#pending = null; callback(value); };
      const timer = setTimeout(() => {
        this.#pending = null;
        reject(new Error('Relay request timeout'));
        // A late reply cannot be assigned to another request after a timeout.
        this.ws.close(CLIENT_PROTOCOL_ERROR, 'Relay Request Timeout');
      }, delay);
      this.#pending = { op: op === 5 ? 200 : op, resolve: finish(resolve), reject: finish(reject) };
      try {
        if (!this.ready) throw new Error('Relay is closed');
        this.ws.send(packet);
      } catch (error) {
        this.#pending.reject(error);
        // A forwarded command may still reply; do not reuse its response slot.
        if (this.ready) this.ws.close(CLIENT_PROTOCOL_ERROR, 'Relay Command Send Failed');
      }
    }));
    this.#controls = result.catch(() => {});
    // Reserve FIFO position before evaluating getters that may enqueue commands.
    try {
      if (!this.ready) throw new Error('Relay is closed');
      if (deferred) captured = structuredClone([RELAY, [op, value]]);
      else packet = encode([RELAY, [op, value]]);
    } catch (error) { encodingFailure = { error }; }
    return result;
  }

  authenticate(password) { return this.request(0, password); }
  listen(flags) { return this.request(1, flags); }
  channels() { return this.request(2, null); }
  async assignId(id) {
    id = asUserId(id);
    if (id === RELAY || id === BROADCAST) throw new RangeError('Reserved session ID');
    return BigInt(await this.request(3, id));
  }
  setVariable(key, value) { return this.request(4, [key, value]); }
  async getVariable(key) { const [found, value] = await this.request(5, key); return { found, value }; }
}
