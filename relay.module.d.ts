type GenericObject = Record<string, unknown>;

export type UserTarget = string | Uint8Array;

export declare const QUERY: Uint8Array;
export declare const BROADCAST: Uint8Array;
export declare const BINARY_TYPE: number;
export declare const TEXT_TYPE: number;

export declare const OP: {
  ANNOUNCE: number;
  ANNOUNCE_REPLY: number;
}

export declare const ArrayEq: (a: array, b: array) => boolean;
export declare const UInt8UserIdToBase64: (userId: Uint8Array) => string;
export declare const Base64ToUInt8UserID: (userId: string) => Uint8Array;
export declare const ArrayToStr: (x: array) => string;
export declare const StrToArray: (x: string) => array;
export declare const UserIdHex: (userId: Uint8Array) => string;

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
  public SendTo(target: UserTarget, msg: GenericObject): void;
  public SendTo(target: UserTarget, opcode: number, msg: ArrayBuffer | Uint8Array): void;
  private bSendTo(target: UserTarget, OpCode: number, msg: ArrayBuffer | Uint8Array): void;
  private tSendTo(target: UserTarget, obj: GenericObject): void;
  public SetChannelVar(key: string, value: string | Uint8Array): void;
  public GetChannelVar(key: string, callback: (message: Uint8Array) => void): void;
}
