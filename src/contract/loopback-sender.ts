import { createSocket, type Socket } from "node:dgram";
import { once } from "node:events";

/** 5 ms at 48 kHz, a house mic sender's packet. */
const FRAMES = 240;
const BYTES_PER_FRAME = 4;
const HEADER_BYTES = 12;
const PAYLOAD_TYPE = 96;

/**
 * A house mic's sender on loopback: RTP L16 packets of silence to an `l16` slot, from a port of
 * its own, so each sender is an address the slot tells apart.
 */
export class LoopbackSender {
  readonly #socket: Socket;
  #ssrc: number;
  #seq = 0;
  #timestamp = 0;

  private constructor(socket: Socket, ssrc: number) {
    this.#socket = socket;
    this.#ssrc = ssrc;
  }

  static async open(ssrc: number) {
    const socket = createSocket("udp4");
    socket.bind(0, "127.0.0.1");
    await once(socket, "listening");
    return new LoopbackSender(socket, ssrc);
  }

  /** Starts a new stream from the same address, as a sender starting over does: a new SSRC. */
  restart() {
    this.#ssrc += 1;
  }

  /** Sends the stream's next packet to `port`, resolving once the kernel has it. */
  async send(port: number) {
    const packet = Buffer.alloc(HEADER_BYTES + FRAMES * BYTES_PER_FRAME);
    packet[0] = 0x80;
    packet[1] = PAYLOAD_TYPE;
    packet.writeUInt16BE(this.#seq, 2);
    packet.writeUInt32BE(this.#timestamp, 4);
    packet.writeUInt32BE(this.#ssrc, 8);
    this.#seq = (this.#seq + 1) & 0xffff;
    this.#timestamp += FRAMES;
    await new Promise<void>((resolve, reject) => {
      this.#socket.send(packet, port, "127.0.0.1", (error) => {
        if (error) reject(error);
        else resolve();
      });
    });
  }

  close() {
    this.#socket.close();
  }
}
