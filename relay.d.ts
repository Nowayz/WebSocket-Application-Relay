type GenericObject = Record<string, unknown>;

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

export type BinaryMessageHandler = (sender: Uint8Array, message: Uint8Array) => void;
export type TextMessageHandler = (sender: Uint8Array, message: GenericObject) => void;
export type VariableCallback = (message: Uint8Array) => void;
export type MessageHandlerCallback = (userId: Uint8Array, message: Uint8Array | GenericObject) => void;

export declare class Relay {
  public ws: WebSocket;
  public onClientDisconnect: ((userId: Uint8Array) => void) | null;
  public onSubscription: ((userId: Uint8Array) => Relay) | null;
  public BinaryMessageHandlers: { [key: number]: BinaryMessageHandler };
  public TextMessageHandlers: { [key: number]: TextMessageHandler };
  public VariableCallbacks: VariableCallback[];
  public channelName: string;
  public ready: boolean;
  public userId: Uint8Array;

  constructor(relayURL: string, channelName: string);
  public JoinChannel(): void;
  public SetMessageHandler(msgType: number, OpCode: number, callback: MessageHandlerCallback): void;
  public MessageDispatcher(e: MessageEvent): void;
  public FirstMessageHandler(e: MessageEvent): void;
  public bSendTo(target: UserTarget, OpCode: number, msg: ArrayBuffer | Uint8Array): void;
  public tSendTo(target: UserTarget, obj: GenericObject): void;
  public SetChannelVar(key: string, value: string | Uint8Array): void;
  public GetChannelVar(key: string, callback: (message: Uint8Array) => void): void;
}
