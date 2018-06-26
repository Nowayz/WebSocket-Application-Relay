# WebSocket Application Relay

## How To
WebSocket Application Relay is designed to be serverless, allowing communications directly between web-clients using **relay.js** library.

It is also possible to have a 'server client' with a fixed UserId, which communicates with regular clients and performs whatever authoritative communications need to occur.

## API Outline (relay.js)
Coming soon...


## Technical Outline
WebSocket Application Relay allows applications to define their own protocol and communicate using their protocol by sending messages to channels or other users.

Sending messages to the build-in **RE_BROADCAST_TARGET**  allows the transmission of the message to every user in the same channel.  

It is typical for an application to use channel broadcasting to allow users to discover each other so they may cache each other's UserIds and send private messages directly.

Alternatively, some applications may find it useful to broadcast messages to everybody in the channel if this type of mass communication is what an application requires.

Server users on the relay can authenticate themselves and perform special operations by sending messages to the special **RE_RELAY_TARGET** UserId.




**Built-In UserID Targets**

| Fixed UserID | Value |
|--|--|
|RE_BROADCAST_TARGET | 0xFFFFFFFFFFFFFFFF |
|RE_RELAY_TARGET | 0x0000000000000000 |


## Security Considerations
The relay stores essentially no information about users except what's required to send messages to them, and what channel they are in.  Events users disconnecting are the only events dispatched by the relay itself.

While server clients or traffic inspection clients are great for storing state and dealing with anomalies related to communication, they have no way of inspecting abuses at the protocol or relay level.  Such as: a client rapidly connecting and disconnecting with thousands of sockets without sending any messages.

There will most likely be revisions and additions to what the relay audits. Though where at all possible the relay will do as little as possible - its main goal is forwarding messages.

## Basic Protocol (Internal)
After connecting to the WebSocket Application Relay, the first packet a client sends (<16 characters long), determines which channel they join.

Upon joining a channel, each user is sent an 8-byte UserId to identify their session.  The Id is to identify them to other users, and also allows sending private messages to another user on the relay, regardless their channel. 

After joining a relay channel, a WebSocket connection is tied to the channel for the rest of the socket lifespan.  Talking on two channels at once or changing channels requires the creation of more connections to the relay.

Relay binary messages are all prefixed by 8 byte target UserId. Relay text messages are prefixed by 12 byte base64 encoded UserId which represents an 8 byte UserId.

When joining any channel: UserId will be transmitted in a binary-type message.  A callback is provided (**onSubscription**) when the channel subscription occurs, so that the client may begin talking in the channel.  Conversion between text and binary UserId types occurs automatically by most relay client libraries.

Message-types are not channel specific and any client can send either text or binary messages to other clients.

## Relay: Special Commands

The relay tracks very limited information.  What it does track, it allows authenticated users to query through a request interface.

Authenticated users on the relay may also have the ability to perform certain privileged actions on the relay for the purpose of powering reliable server-based web-applications.

**Information available from the relay:**

 - Current Number of Channels
 - Name of Current Channels
 - Population of Current Channels

**Actions possible on the relay:**

 - Assign custom UserId (disconnects user holding Id if necissary)
