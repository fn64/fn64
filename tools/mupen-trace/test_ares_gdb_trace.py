#!/usr/bin/env python3
"""Offline self-check for ares_gdb_trace: protocol framing + decode, no ares.

Runs a mock GDB server on a socket that answers the exact packets ares
would, drives one breakpoint hit, and asserts the emitted JSONL carries
the right site->target edge. Also unit-checks the jr/jalr decoder and the
packet checksum. `python3 test_ares_gdb_trace.py` exits 0 on success.
"""

import json
import socket
import tempfile
import threading
import os

import ares_gdb_trace as A


def test_decode_indirect():
    # jr $ra  = 0x03e00008: SPECIAL, rs=31, funct=0x08
    assert A.decode_indirect(0x03E00008) == (31, False)
    # jalr $ra,$t9 = 0x0320f809: rs=25 (t9), funct=0x09
    assert A.decode_indirect(0x0320F809) == (25, True)
    # addiu (op 9) is not indirect
    assert A.decode_indirect(0x24020001) is None
    # sll/nop (op 0, funct 0) is not a jump
    assert A.decode_indirect(0x00000000) is None
    print("decode_indirect ok")


def test_checksum_and_frame():
    # "OK" checksum = (0x4F+0x4B) & 0xFF = 0x9a
    assert A.GdbClient._checksum(b"OK") == 0x9A
    print("checksum ok")


def _mock_server(conn, target_value):
    """Answer the client's packets like ares would for one break + reads."""
    buf = b""

    def recv_packet():
        nonlocal buf
        while b"#" not in buf:
            chunk = conn.recv(4096)
            if not chunk:
                return None
            buf += chunk
        dollar = buf.index(b"$")
        hash_i = buf.index(b"#", dollar)
        payload = buf[dollar + 1 : hash_i]
        buf = buf[hash_i + 3 :]  # drop payload + 2 checksum chars
        return payload.decode()

    def send_packet(payload: str):
        body = payload.encode()
        conn.sendall(b"$" + body + b"#" + f"{sum(body) & 0xFF:02x}".encode())

    site_pc = 0x80193EFC
    stopped_once = False
    while True:
        pkt = recv_packet()
        if pkt is None:
            return
        conn.sendall(b"+")  # ack the client's packet
        if pkt == "?":
            send_packet("T05")
        elif pkt.startswith("Z0"):
            send_packet("OK")
        elif pkt.startswith("z0"):
            send_packet("OK")
        elif pkt == "c":
            send_packet("OK")
            if not stopped_once:
                stopped_once = True
                send_packet("T05")  # immediately "hit" the breakpoint
        elif pkt.startswith("p"):
            idx = int(pkt[1:], 16)
            if idx == A.PC_REG_INDEX:
                send_packet(f"{site_pc:016x}")
            elif idx == 25:  # the site's src_reg
                send_packet(f"{target_value:016x}")
            else:
                send_packet("0" * 16)
        else:
            send_packet("")


def test_end_to_end_edge_capture():
    # Minimal fake ROM: native z64 magic + one jalr $t9 at file offset 0.
    rom_path = tempfile.mktemp(suffix=".z64")
    with open(rom_path, "wb") as f:
        f.write(b"\x80\x37\x12\x40" + b"\x00" * 0x40)

    sites_path = tempfile.mktemp(suffix=".json")
    json.dump([{"site": "0x80193efc", "src_reg": 25, "via_call": True}], open(sites_path, "w"))
    out_path = tempfile.mktemp(suffix=".jsonl")

    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    srv.listen(1)
    port = srv.getsockname()[1]

    target = 0x80194000

    def serve():
        conn, _ = srv.accept()
        _mock_server(conn, target)

    t = threading.Thread(target=serve, daemon=True)
    t.start()

    args = type("A", (), {})()
    args.rom, args.sites, args.out = rom_path, sites_path, out_path
    args.trace_id, args.bank = "test-1", "request_dma_0"
    args.host, args.port = "127.0.0.1", port
    args.duration, args.poll = 2.0, 0.2
    A.run(args)

    lines = [json.loads(l) for l in open(out_path)]
    header = lines[0]
    assert header["event"] == "header" and header["schema_version"] == 1
    edges = [l for l in lines if l["event"] == "indirect_transfer"]
    assert len(edges) == 1, edges
    e = edges[0]
    assert e["kind"] == "call"
    assert e["site"]["address"] == 0x80193EFC
    assert e["target"]["address"] == target
    assert e["site"]["bank"]["bank"] == "request_dma_0"
    assert lines[-1]["event"] == "end"

    for p in (rom_path, sites_path, out_path):
        os.unlink(p)
    print("end_to_end edge capture ok")


if __name__ == "__main__":
    test_decode_indirect()
    test_checksum_and_frame()
    test_end_to_end_edge_capture()
    print("ALL PASSED")
