// The least of RFC 6455 a one-way feed needs: the upgrade, unmasked text frames out, and close on the client's close.
import { createHash } from "node:crypto";
import { Buffer } from "node:buffer";

const GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/** Completes the upgrade on `socket` and returns `send(text)` and `close()`. */
export function accept(request, socket, onClose) {
  const key = request.headers["sec-websocket-key"];
  const answer = createHash("sha1").update(`${key}${GUID}`).digest("base64");
  socket.write(
    `HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${answer}\r\n\r\n`,
  );
  socket.on("data", (chunk) => {
    // A client close frame has opcode 8; it is all this one-way feed listens for.
    if ((chunk[0] & 0x0f) === 0x8) {
      socket.end();
    }
  });
  socket.on("close", onClose);
  socket.on("error", () => {
    socket.destroy();
  });
  return {
    send(message) {
      const body = Buffer.from(message);
      let header;
      if (body.length < 126) {
        header = Buffer.from([0x81, body.length]);
      } else if (body.length < 65536) {
        header = Buffer.from([0x81, 126, body.length >> 8, body.length & 0xff]);
      } else {
        header = Buffer.alloc(10);
        header[0] = 0x81;
        header[1] = 127;
        header.writeBigUInt64BE(BigInt(body.length), 2);
      }
      socket.write(Buffer.concat([header, body]));
    },
    close() {
      socket.end();
    },
  };
}
