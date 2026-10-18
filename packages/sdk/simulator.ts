/**
 * Toolkit for protocol-level simulators (ADR 0012): fake device servers that
 * the real plugin code connects to. They double as the plugin's test suite.
 *
 * ```ts
 * const server = serveTcp(async (client) => {
 *   await client.write("HELLO\n");
 *   for await (const line of client.lines()) await client.write(`ECHO ${line}\n`);
 * });
 * // connect the plugin to 127.0.0.1:${server.port} …
 * await server.close();
 * ```
 *
 * @module
 */

const encoder = new TextEncoder();

/** One client connected to a fake server. */
export class TcpClient {
  readonly #conn: Deno.TcpConn;
  #closed = false;
  #writes: Promise<void> = Promise.resolve();

  constructor(conn: Deno.TcpConn) {
    this.#conn = conn;
  }

  get closed(): boolean {
    return this.#closed;
  }

  /** Writes text or bytes. Writes go out in call order. */
  write(data: string | Uint8Array): Promise<void> {
    const bytes = typeof data === "string" ? encoder.encode(data) : data;
    this.#writes = this.#writes.then(async () => {
      if (this.#closed) return;
      let off = 0;
      try {
        while (off < bytes.length) {
          off += await this.#conn.write(bytes.subarray(off));
        }
      } catch {
        this.close();
      }
    });
    return this.#writes;
  }

  /** Raw chunks until the client disconnects. */
  async *chunks(): AsyncGenerator<Uint8Array> {
    const buf = new Uint8Array(16 * 1024);
    while (!this.#closed) {
      let n: number | null;
      try {
        n = await this.#conn.read(buf);
      } catch {
        n = null;
      }
      if (n === null) break;
      yield buf.slice(0, n);
    }
    this.close();
  }

  /** Lines (without `\n` or `\r\n`) until the client disconnects. */
  async *lines(): AsyncGenerator<string> {
    const decoder = new TextDecoder();
    let text = "";
    for await (const chunk of this.chunks()) {
      text += decoder.decode(chunk, { stream: true });
      let nl: number;
      while ((nl = text.indexOf("\n")) >= 0) {
        yield text.slice(0, nl).replace(/\r$/, "");
        text = text.slice(nl + 1);
      }
    }
  }

  close() {
    if (this.#closed) return;
    this.#closed = true;
    try {
      this.#conn.close();
    } catch {
      // already closed
    }
  }
}

/** A running fake TCP server. */
export interface TcpSimulator {
  readonly hostname: string;
  readonly port: number;
  readonly clients: ReadonlySet<TcpClient>;
  /** Writes to every connected client. */
  broadcast(data: string | Uint8Array): Promise<void>;
  /** Drops every client, as a device reboot would; keeps listening. */
  dropClients(): void;
  /** Stops listening, drops every client and waits for their handlers. */
  close(): Promise<void>;
}

/**
 * Starts a fake TCP server. `handler` runs once per client; the client is
 * closed when it returns.
 */
export function serveTcp(
  handler: (client: TcpClient) => void | Promise<void>,
  options: { hostname?: string; port?: number } = {},
): TcpSimulator {
  const hostname = options.hostname ?? "127.0.0.1";
  const listener = Deno.listen({ hostname, port: options.port ?? 0 });
  const clients = new Set<TcpClient>();
  const handlers = new Set<Promise<void>>();

  const accept = (async () => {
    for (;;) {
      let conn: Deno.TcpConn;
      try {
        conn = await listener.accept();
      } catch {
        return; // listener closed
      }
      const client = new TcpClient(conn);
      clients.add(client);
      const run = (async () => {
        try {
          await handler(client);
        } catch {
          // a handler error drops that client only
        } finally {
          client.close();
          clients.delete(client);
        }
      })();
      handlers.add(run);
      run.finally(() => handlers.delete(run));
    }
  })();

  return {
    hostname,
    port: (listener.addr as Deno.NetAddr).port,
    clients,
    async broadcast(data) {
      await Promise.all([...clients].map((c) => c.write(data)));
    },
    dropClients() {
      for (const c of clients) c.close();
    },
    async close() {
      listener.close();
      for (const c of clients) c.close();
      await accept;
      await Promise.all([...handlers]);
    },
  };
}
