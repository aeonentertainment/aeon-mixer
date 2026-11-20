import { createSocket, type Socket } from "node:dgram";
import { once } from "node:events";

/** 5 ms at 48 kHz, a house mic sender's packet. */
const FRAMES = 240;
const BYTES_PER_FRAME = 4;
const HEADER_BYTES = 12;
const PAYLOAD_TYPE = 96;

/**
 * A sender on this machine: RTP L16 packets of silence from a port of its own on `address`, to a
 * slot's port at that same address, so each sender is an address the slot tells apart. On
 * loopback it is a house mic's sender to an `l16` slot.
 *
 * Every sender starts on the same SSRC, because the lock goes by address, never by SSRC
 * (decision 0028). A slot that lost its lock then takes another sender's packets as late ones on
 * the stream it has, and sees only a `restart()` as a new stream, so a check that waits for a
 * resync cannot end early on a broken lock.
 */
export class LocalSender {
  readonly #socket: Socket;
  readonly #address: string;
  #ssrc = 1;
  #seq = 0;
  #timestamp = 0;

  private constructor(socket: Socket, address: string) {
    this.#socket = socket;
    this.#address = address;
  }

  /** Binds a port of the OS's choice on `address`, an IPv4 address of this machine. */
  static async open(address = "127.0.0.1") {
    const socket = createSocket("udp4");
    socket.bind(0, address);
    await once(socket, "listening");
    return new LocalSender(socket, address);
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
      this.#socket.send(packet, port, this.#address, (error) => {
        if (error) reject(error);
        else resolve();
      });
    });
  }

  close() {
    this.#socket.close();
  }
}
