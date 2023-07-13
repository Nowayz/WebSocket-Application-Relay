export declare const re: {
  BINARY_BROADCAST: Uint8Array,
  RELAY_QUERY: Uint8Array,
  TEXT_BROADCAST: string,
  BINARY: number,
  TEXT: number,
  OP: {
    ANNOUNCE: number,
    ANNOUNCE_REPLY: number,
  },
  USERS: {},
  UInt8UserIdToBase64: (userId: Uint8Array) => string,
  Base64ToUInt8UserID: (userId: string) => Uint8Array,
  ArrayToStr: (x: array) => string,
  StrToArray: (x: string) => array,
  UserIdHex: (userId: Uint8Array) => string,
};

export declare const ArrayEq: (a: array, b: array) => boolean;
export declare const UInt8UserIdToBase64: (userId: Uint8Array) => string;
export declare const Base64ToUInt8UserID: (userId: string) => Uint8Array;

export declare class Relay {
  public ws: WebSocket;
  public onClientDisconnect: ((userId: Uint8Array) => void) | null;
  public onSubscription: ((userId: Uint8Array) => Relay) | null;
  public BinaryMessageHandlers: { [key: keyof re.OP]: (sender: Uint8Array, message: Uint8Array | object) => void };
  public TextMessageHandlers: { [key: keyof re.OP]: (sender: Uint8Array, message: Uint8Array | object) => void };
  public VariableCallbacks: Function[];
  public channelName: string;
  public ready: boolean;
  public userId: Uint8Array;

  constructor(relayURL: string, channelName: string);
  public JoinChannel(): void;
  public SetMessageHandler(msgType: number, OpCode: keyof re.OP, callback: unknown): void;
  public MessageDispatcher(e: MessageEvent): void;
  public FirstMessageHandler(e: MessageEvent): void;
  public bSendTo(target: Uint8Array, OpCode: keyof re.OP, msg: any): void;
  public tSendTo(target: string | Uint8Array, obj: any): void;
  public SetChannelVar(key: string, value: any): void;
  public GetChannelVar(key: string, callback: Function): void;
}
