import { Encoder, Decoder } from 'cbor-x';

export const BROADCAST = 0xffffffffffffffffn;
export const RELAY = 0n;
export const LISTEN = Object.freeze({ CHANNEL: 1, PRIVATE: 2, DISCONNECT: 4 });

// Standard CBOR only: record-extension tags would bind the server to this codec.
const encoder = new Encoder({ useRecords: false, mapsAsObjects: true, tagUint8Array: false });
const decoder = new Decoder({ mapsAsObjects: true });

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

/** Browser/Node 22+ client. All IDs stay BigInt, including small fixed IDs. */
export class Relay extends EventTarget {
  #pending = null;
  #controls = Promise.resolve();

  constructor(ws, userId, requestTimeoutMs) {
    super();
    this.ws = ws;
    this.userId = wireId(userId, true);
    this.requestTimeoutMs = requestTimeoutMs;
    ws.addEventListener('message', event => {
      try {
        const [rawSender, value] = decoder.decode(new Uint8Array(event.data));
        const sender = wireId(rawSender);
        if (sender === RELAY) {
          const [op, result] = value;
          if (!this.#pending || this.#pending.op !== op) throw new Error('Unexpected relay response');
          if (op === 3) wireId(result, true);
          this.#pending.resolve(result);
        } else if (sender === BROADCAST) {
          this.dispatchEvent(new CustomEvent('disconnect', { detail: { userId: wireId(value, true) } }));
        } else {
          this.dispatchEvent(new CustomEvent('message', { detail: { sender, value } }));
        }
      } catch (error) {
        this.dispatchEvent(new CustomEvent('error', { detail: error }));
        ws.close(1002, 'Invalid Relay Response');
      }
    });
    ws.addEventListener('close', event => {
      this.#pending?.reject(new Error(`Relay closed (${event.code}): ${event.reason}`));
      this.dispatchEvent(new CustomEvent('close', { detail: { code: event.code, reason: event.reason } }));
    });
  }

  static async connect(url, channel, { requestTimeoutMs = 10_000, WebSocketClass = globalThis.WebSocket } = {}) {
    if (!(requestTimeoutMs > 0 && Number.isFinite(requestTimeoutMs))) throw new RangeError('requestTimeoutMs must be positive');
    const channelBytes = typeof channel === 'string' ? new TextEncoder().encode(channel) : channel;
    if (!(channelBytes instanceof Uint8Array) || channelBytes.length > 16) throw new RangeError('Channel must contain at most 16 bytes');
    const ws = new WebSocketClass(url, 'war.cbor.v1');
    ws.binaryType = 'arraybuffer';
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { reject(new Error('Relay join timeout')); ws.close(); }, requestTimeoutMs);
      const fail = () => { clearTimeout(timer); reject(new Error('Relay closed before subscription')); };
      ws.addEventListener('close', fail, { once: true });
      ws.addEventListener('error', fail, { once: true });
      ws.addEventListener('open', () => ws.send(encoder.encode(channel)), { once: true });
      ws.addEventListener('message', event => {
        clearTimeout(timer); ws.removeEventListener('close', fail); ws.removeEventListener('error', fail);
        try {
          const id = decoder.decode(new Uint8Array(event.data));
          resolve(new Relay(ws, id, requestTimeoutMs));
        } catch (error) { reject(error); ws.close(1002, 'Invalid Subscription'); }
      }, { once: true });
    });
  }

  get ready() { return this.ws.readyState === 1; }

  send(target, value) {
    if (!this.ready) throw new Error('Relay is closed');
    // Number inputs larger than the safe-integer range have already lost precision.
    target = asUserId(target);
    this.ws.send(encoder.encode([target, value]));
  }

  broadcast(value) { this.send(BROADCAST, value); }
  close() { this.ws.close(1000); }

  /** Serialize commands: the deliberately small wire protocol has no request IDs. */
  request(op, value) {
    const result = this.#controls.then(() => new Promise((resolve, reject) => {
      const finish = callback => value => { clearTimeout(timer); this.#pending = null; callback(value); };
      const timer = setTimeout(() => {
        this.#pending = null;
        reject(new Error('Relay request timeout'));
        // A late reply cannot be assigned to another request after a timeout.
        this.ws.close(1002, 'Relay Request Timeout');
      }, this.requestTimeoutMs);
      this.#pending = { op: op === 5 ? 200 : op, resolve: finish(resolve), reject: finish(reject) };
      try { this.send(RELAY, [op, value]); } catch (error) { this.#pending.reject(error); }
    }));
    this.#controls = result.catch(() => {});
    return result;
  }

  authenticate(password) { return this.request(0, password); }
  listen(flags) { return this.request(1, flags); }
  channels() { return this.request(2, null); }
  async assignId(id) {
    id = asUserId(id);
    if (id === RELAY || id === BROADCAST) throw new RangeError('Reserved session ID');
    this.userId = BigInt(await this.request(3, id));
    return this.userId;
  }
  setVariable(key, value) { return this.request(4, [key, value]); }
  async getVariable(key) { const [found, value] = await this.request(5, key); return { found, value }; }
}
