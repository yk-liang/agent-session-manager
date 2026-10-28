import { createServer } from 'node:net';
import { once } from 'node:events';

// Vite 7 interprets port=0 as its default dev port. Reserve an OS-assigned port
// first, then let strictPort fail explicitly if another process wins the race.
export async function availablePort() {
  const socket = createServer();
  socket.listen(0, '127.0.0.1');
  await once(socket, 'listening');
  const { port } = socket.address();
  await new Promise((resolve, reject) => socket.close(error => error ? reject(error) : resolve()));
  return port;
}
