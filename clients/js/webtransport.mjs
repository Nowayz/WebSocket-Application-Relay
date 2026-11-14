// Event facade keeps the relay's command serialization and CBOR validation shared.
export class WebTransportChannel extends EventTarget {
  readyState = 0;
  #writer;
  #datagrams;
  #writes = Promise.resolve();
  #closed = false;
  #closing;
  #closeTimer;
  #closeTimeoutMs;
  constructor(url, { WebTransportClass = globalThis.WebTransport, closeTimeoutMs = 10_000, ...options } = {}) {
    super();
    this.#closeTimeoutMs = closeTimeoutMs;
    if (!WebTransportClass) throw new Error('WebTransport is unavailable');
    this.transport = new WebTransportClass(url, options);
    this.transport.closed.then(info => this.#finish(info.closeCode ?? 0, info.reason ?? ''), error => this.#fail(error));
    this.#open().catch(error => this.#fail(error));
  }
  async #open() {
    await this.transport.ready;
    const stream = await this.transport.createBidirectionalStream();
    this.#writer = stream.writable.getWriter();
    const datagrams = this.transport.datagrams;
    this.#datagrams = (typeof datagrams.createWritable === 'function' ? datagrams.createWritable() : datagrams.writable).getWriter();
    if (this.#closed) return;
    this.readyState = 1;
    this.dispatchEvent(new Event('open'));
    await Promise.all([this.#readRecords(stream.readable), this.#readDatagrams(datagrams.readable)]);
  }
  async #readRecords(readable) {
    const reader = readable.getReader();
    const header = new Uint8Array(4);
    let headerBytes = 0, size = 0, body, used = 0;
    try {
      for (;;) {
        const { value, done } = await reader.read();
        if (done) {
          if (headerBytes || body) throw new Error('Truncated relay record');
          this.close(); return;
        }
        let offset = 0;
        while (offset < value.length) {
          if (!body) {
            const count = Math.min(4 - headerBytes, value.length - offset);
            header.set(value.subarray(offset, offset + count), headerBytes);
            headerBytes += count; offset += count;
            if (headerBytes !== 4) continue;
            size = new DataView(header.buffer).getUint32(0);
            if (size === 0) {
              if (!this.#closing) throw new Error('Unexpected close acknowledgement');
              this.transport.close(this.#closing);
              return;
            }
            // Grow only as bytes arrive, never allocate an untrusted announced size.
            body = new Uint8Array(Math.min(size, 4096)); used = 0;
          }
          const count = Math.min(size - used, value.length - offset);
          if (body.length < used + count) {
            const grown = new Uint8Array(Math.min(size, Math.max(used + count, body.length * 2)));
            grown.set(body); body = grown;
          }
          body.set(value.subarray(offset, offset + count), used);
          offset += count; used += count;
          if (used === size) {
            this.#message(body, false);
            body = undefined; headerBytes = 0;
          }
        }
      }
    } finally { reader.releaseLock(); }
  }
  async #readDatagrams(readable) {
    const reader = readable.getReader();
    try {
      for (;;) {
        const { value, done } = await reader.read();
        if (done) return;
        this.#message(value, true);
      }
    } finally { reader.releaseLock(); }
  }
  #message(data, unreliable) {
    const event = new MessageEvent('message', { data });
    Object.defineProperty(event, 'unreliable', { value: unreliable });
    this.dispatchEvent(event);
  }
  send(bytes) {
    if (this.readyState !== 1) throw new Error('Relay transport is closed');
    const packet = new Uint8Array(4 + bytes.length);
    new DataView(packet.buffer).setUint32(0, bytes.length);
    packet.set(bytes, 4);
    this.#writes = this.#writes.then(() => this.#writer.write(packet));
    this.#writes.catch(error => this.#fail(error));
  }
  get maxDatagramSize() { return this.transport.datagrams.maxDatagramSize; }
  async sendUnreliable(bytes) {
    if (this.readyState !== 1) throw new Error('Relay transport is closed');
    if (bytes.length > this.maxDatagramSize) throw new RangeError('Message exceeds negotiated datagram size');
    // Own encoder storage before waiting for congestion-control backpressure.
    const packet = Uint8Array.from(bytes);
    await this.#datagrams.ready;
    if (this.readyState !== 1) throw new Error('Relay transport is closed');
    await this.#datagrams.write(packet);
  }
  close(code = 0, reason = '') {
    if (this.#closed || (this.#closing && code === 1000)) return;
    const wasReady = this.readyState === 1;
    this.readyState = 2;
    if (code !== 1000 || !wasReady) {
      this.transport.close({ closeCode: code, reason });
      return;
    }
    if (this.#closing) return;
    this.#closing = { closeCode: code, reason };
    this.#closeTimer = setTimeout(() => this.#fail(new Error('Graceful close timeout')), this.#closeTimeoutMs);
    this.#writes = this.#writes.then(() => this.#writer.write(new Uint8Array(4)));
    this.#writes.catch(error => this.#fail(error));
  }
  #finish(code, reason) {
    if (this.#closed) return;
    clearTimeout(this.#closeTimer);
    this.#closed = true; this.readyState = 3;
    const event = new Event('close');
    Object.assign(event, { code, reason });
    this.dispatchEvent(event);
  }
  #fail(error) {
    if (this.#closed) return;
    this.dispatchEvent(new CustomEvent('error', { detail: error }));
    this.close(4002, 'WebTransport failed');
    this.#finish(4002, 'WebTransport failed');
  }
}
