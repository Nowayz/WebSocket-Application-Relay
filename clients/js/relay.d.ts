export const BROADCAST: 18446744073709551615n;
export const RELAY: 0n;
export const LISTEN: Readonly<{ CHANNEL: 1; PRIVATE: 2; DISCONNECT: 4 }>;

/** Use bigint for full uint64 IDs; numeric inputs must be safe integers. */
export type UserId = bigint | number;
export type Channel = string | Uint8Array;
export type VariableResult<T = unknown> = { found: false; value: null } | { found: true; value: T };
export type ChannelListing = Array<[Uint8Array, number | bigint]>;
export interface RelayOptions {
  requestTimeoutMs?: number;
  WebSocketClass?: new (url: string | URL, protocols?: string | string[]) => WebSocket;
}
export interface RelayEventMap {
  message: CustomEvent<{ sender: bigint; value: unknown }>;
  disconnect: CustomEvent<{ userId: bigint }>;
  close: CustomEvent<{ code: number; reason: string }>;
  error: CustomEvent<unknown>;
}

/** Browser and Node 22+ client for war.cbor.v1. All received IDs are bigint. */
export class Relay extends EventTarget {
  constructor(ws: WebSocket, userId: UserId, requestTimeoutMs?: number);
  static connect(url: string | URL, channel: Channel, options?: RelayOptions): Promise<Relay>;
  readonly ws: WebSocket;
  readonly userId: bigint;
  readonly ready: boolean;
  requestTimeoutMs: number;
  send(target: UserId, value: unknown): void;
  broadcast(value: unknown): void;
  close(): void;
  request(op: number, value: unknown): Promise<unknown>;
  authenticate(password: string | Uint8Array): Promise<boolean>;
  listen(flags: number): Promise<number | bigint>;
  channels(): Promise<ChannelListing>;
  assignId(id: UserId): Promise<bigint>;
  setVariable(key: string | Uint8Array, value: unknown): Promise<boolean>;
  getVariable<T = unknown>(key: string | Uint8Array): Promise<VariableResult<T>>;
  addEventListener<K extends keyof RelayEventMap>(type: K,
    callback: ((this: Relay, event: RelayEventMap[K]) => void) | null,
    options?: boolean | AddEventListenerOptions): void;
  addEventListener(type: string, callback: EventListenerOrEventListenerObject | null,
    options?: boolean | AddEventListenerOptions): void;
  removeEventListener<K extends keyof RelayEventMap>(type: K,
    callback: ((this: Relay, event: RelayEventMap[K]) => void) | null,
    options?: boolean | EventListenerOptions): void;
  removeEventListener(type: string, callback: EventListenerOrEventListenerObject | null,
    options?: boolean | EventListenerOptions): void;
}
