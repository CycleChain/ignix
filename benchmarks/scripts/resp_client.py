"""Minimal RESP client shared by the benchmark scripts.

Every connection keeps one buffered reader, bulk replies are read to their
full declared length and error replies raise `RespError`, so a benchmark can
tell a correct reply from an error, a short read or a closed connection.
Only the standard library is used.
"""

import socket
from typing import List, Optional, Union

Reply = Union[bytes, int, None, List["Reply"]]


class RespError(Exception):
    """The server answered with an error reply (`-ERR ...`)."""


class ProtocolError(Exception):
    """The server sent something that is not a RESP reply."""


def _as_bytes(arg) -> bytes:
    if isinstance(arg, bytes):
        return arg
    if isinstance(arg, str):
        return arg.encode("utf-8")
    return str(arg).encode("ascii")


def encode_command(*args) -> bytes:
    """Encode a command as a RESP array of bulk strings."""
    parts = [b"*%d\r\n" % len(args)]
    for arg in args:
        data = _as_bytes(arg)
        parts.append(b"$%d\r\n" % len(data))
        parts.append(data)
        parts.append(b"\r\n")
    return b"".join(parts)


class RespClient:
    """One blocking connection that sends commands and reads replies."""

    def __init__(self, host: str, port: int, timeout: float = 5.0):
        self.host = host
        self.port = port
        self.timeout = timeout
        self.sock: Optional[socket.socket] = None
        self.reader = None

    def connect(self) -> None:
        self.sock = socket.create_connection((self.host, self.port), timeout=self.timeout)
        self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self.reader = self.sock.makefile("rb", buffering=65536)

    def close(self) -> None:
        for resource in (self.reader, self.sock):
            if resource is not None:
                try:
                    resource.close()
                except OSError:
                    pass
        self.reader = None
        self.sock = None

    def __enter__(self) -> "RespClient":
        self.connect()
        return self

    def __exit__(self, *exc) -> None:
        self.close()

    def read_reply(self) -> Reply:
        """Read one complete reply; raises RespError for error replies."""
        line = self.reader.readline()
        if not line.endswith(b"\r\n"):
            raise ConnectionError("connection closed while reading a reply")
        kind, body = line[:1], line[1:-2]
        if kind == b"+":
            return body
        if kind == b"-":
            raise RespError(body.decode("utf-8", errors="replace"))
        if kind == b":":
            return int(body)
        if kind == b"$":
            length = int(body)
            if length < 0:
                return None
            data = self.reader.read(length + 2)
            if len(data) != length + 2 or not data.endswith(b"\r\n"):
                raise ConnectionError("connection closed inside a bulk reply")
            return data[:-2]
        if kind == b"*":
            count = int(body)
            if count < 0:
                return None
            return [self.read_reply() for _ in range(count)]
        raise ProtocolError(f"unexpected reply {line[:40]!r}")

    def request(self, payload: bytes) -> Reply:
        """Send an already encoded command and return its reply."""
        self.sock.sendall(payload)
        return self.read_reply()

    def execute(self, *args) -> Reply:
        """Encode and send a command and return its reply."""
        return self.request(encode_command(*args))


def ping(host: str, port: int, timeout: float = 2.0) -> bool:
    """Whether a RESP server answers PING on host:port."""
    try:
        with RespClient(host, port, timeout) as client:
            return client.execute("PING") == b"PONG"
    except (OSError, RespError, ProtocolError, ValueError):
        return False
