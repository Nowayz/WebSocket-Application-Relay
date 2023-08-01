type GenericObject = Record<string, unknown>;
type GenericFunction = (...args: unknown[]) => unknown;

export declare const re: {
  BINARY_BROADCAST: Uint8Array,
  RELAY_QUERY: Uint8Array,
  TEXT_BROADCAST: string,
  BINARY: number,
  TEXT: number,
  USERS: {}, 
  UInt8UserIdToBase64: (userId: Uint8Array) => string,
  Base64ToUInt8UserID: (userId: string) => Uint8Array,
  ArrayToStr: (x: array) => string,
  StrToArray: (x: string) => array,
  UserIdHex: (userId: Uint8Array) => string,
  [key: string]: unknown,
};

export declare const ArrayEq: (a: array, b: array) => boolean;
export declare const UInt8UserIdToBase64: (userId: Uint8Array) => string;
export declare const Base64ToUInt8UserID: (userId: string) => Uint8Array;

export declare class Relay {
  public ws: WebSocket;
  public onClientDisconnect: ((userId: Uint8Array) => unknown) | null;
  public onSubscription: ((userId: Uint8Array) => Relay) | null;
  public BinaryMessageHandlers: { [key: number]: (sender: Uint8Array, message: Uint8Array) => unknown };
  public TextMessageHandlers: { [key: number]: (sender: Uint8Array, message: GenericObject) => unknown };
  public VariableCallbacks: GenericFunction[];
  public channelName: string;
  public ready: boolean;
  public userId: Uint8Array;

  constructor(relayURL: string, channelName: string);
  public JoinChannel(): void;
  public SetMessageHandler(msgType: number, OpCode: number, callback: unknown): void;
  public MessageDispatcher(e: MessageEvent): void;
  public FirstMessageHandler(e: MessageEvent): void;
  public bSendTo(target: Uint8Array, OpCode: number, msg: any): void;
  public tSendTo(target: string | Uint8Array, obj: any): void;
  public SetChannelVar(key: string, value: any): void;
  public GetChannelVar(key: string, callback: GenericFunction): void;
}
