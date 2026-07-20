#!/usr/bin/env python3
"""Capture real indirect-transfer edges from ares over its GDB stub.

ares (v148, ISC) exposes a standard GDB Remote Serial Protocol server on a
TCP port (default 9123) when launched with Boot/AwaitGDBClient = true. Its
CPU-accurate core free-runs at recompiler speed and only breaks on the PCs
we install, so this reaches real gameplay — the thing mupen64plus's
single-step debugger could not do (see docs/DISCOVER-PLAN.md and the
project memory).

This driver is the automatable half of the trace-producer. A HUMAN must
first launch ares in a real desktop (GUI) session on the target ROM with
the GDB stub enabled — ares has no headless mode, so an agent cannot do
that step. Once ares is running and waiting for a client, run:

    python3 ares_gdb_trace.py \
        --rom "/path/Majora's Mask (USA).z64" \
        --sites sites.json \
        --out mm.trace.jsonl \
        --trace-id mm-ares-1 \
        [--host 127.0.0.1] [--port 9123] [--duration 120]

`sites.json` is a list of indirect-transfer sites to watch, each a `jr`/
`jalr` instruction whose target is the runtime value of a source GPR:

    [{"site": "0x80193efc", "src_reg": 25, "via_call": true}, ...]

(Derive it with --emit-sites, which decodes the ROM's open-frontier PCs;
see build_sites() below.) On each breakpoint hit the driver reads the
source register and emits one fn64 trace-schema `indirect_transfer`
record: site = the breakpoint PC, target = the register value. No
single-stepping is needed — the target IS the source register's value at
the branch.

The output is exactly the JSONL that `gate_decomp_functions`'
FN64_DISCOVER_TRACE env and `trace::fold_indirect_targets_into_fact_db`
consume; the observed targets become excused code-segment owner roots.

Register map (from ares/n64/system/system.cpp regRead hook): GPR $rN =
index N (0-31), $pc = index 37, each 16 hex chars, 64-bit big-endian.
"""

import argparse
import hashlib
import json
import socket
import sys
import time

# --- GDB Remote Serial Protocol (the minimal subset ares implements) ---

PC_REG_INDEX = 37


class GdbClient:
    def __init__(self, host, port, timeout=5.0):
        self.sock = socket.create_connection((host, port), timeout=timeout)
        self.sock.settimeout(timeout)
        self.buf = b""

    def close(self):
        try:
            self.sock.close()
        except OSError:
            pass

    @staticmethod
    def _checksum(payload: bytes) -> int:
        return sum(payload) & 0xFF

    def _send_raw(self, data: bytes):
        self.sock.sendall(data)

    def send_packet(self, payload: str):
        body = payload.encode("ascii")
        pkt = b"$" + body + b"#" + f"{self._checksum(body):02x}".encode("ascii")
        self._send_raw(pkt)
        self._expect_ack()

    def _read_byte(self) -> bytes:
        while not self.buf:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise ConnectionError("ares closed the GDB connection")
            self.buf += chunk
        b, self.buf = self.buf[:1], self.buf[1:]
        return b

    def _expect_ack(self):
        # ares acks each packet with '+'. Tolerate a stray '-' (resend) by
        # surfacing it loudly rather than silently hanging.
        b = self._read_byte()
        if b == b"+":
            return
        if b == b"-":
            raise IOError("ares NAK'd a packet (resend requested)")
        # Some stacks interleave an unsolicited stop-reply; push back.
        self.buf = b + self.buf

    def read_packet(self, timeout=None) -> str:
        """Read one `$...#xx` packet, ack it, return the payload."""
        if timeout is not None:
            self.sock.settimeout(timeout)
        try:
            while True:
                b = self._read_byte()
                if b == b"$":
                    break
                # '+'/'-' acks or noise between packets: skip.
            body = b""
            while True:
                b = self._read_byte()
                if b == b"#":
                    break
                body += b
            self._read_byte()  # checksum hi
            self._read_byte()  # checksum lo
            self._send_raw(b"+")
            return body.decode("ascii", "replace")
        except socket.timeout:
            return ""
        finally:
            if timeout is not None:
                self.sock.settimeout(5.0)

    # --- high-level ops ---

    def handshake(self):
        # ares sends nothing until we speak; '?' asks why we halted and
        # returns a faked T05 so the client does not spin.
        self.send_packet("?")
        return self.read_packet()

    def set_breakpoint(self, addr: int):
        self.send_packet(f"Z0,{addr:x},4")
        return self.read_packet()

    def remove_breakpoint(self, addr: int):
        self.send_packet(f"z0,{addr:x},4")
        return self.read_packet()

    def read_reg(self, index: int) -> int:
        self.send_packet(f"p{index:x}")
        reply = self.read_packet()
        if not reply or reply.startswith("E"):
            raise IOError(f"register {index} read failed: {reply!r}")
        # 16 hex chars, big-endian 64-bit.
        return int(reply[:16], 16)

    def cont(self):
        """Resume. ares replies OK, then pushes a T05 stop-reply on halt."""
        self.send_packet("c")
        # 'c' returns "OK" immediately in stop-mode; the stop-reply arrives
        # later, so don't block for it here.
        return self.read_packet(timeout=0.2)

    def wait_for_stop(self, timeout: float) -> str:
        """Block until a stop-reply (T05/S05) arrives, or timeout ('' )."""
        return self.read_packet(timeout=timeout)


# --- ROM decode: derive jr/jalr sites from open-frontier PCs ---


def _u32(rom: bytes, off: int) -> int:
    return int.from_bytes(rom[off : off + 4], "big")


def decode_indirect(word: int):
    """Return (src_reg, via_call) if `word` is jr/jalr, else None.

    MIPS: SPECIAL (op 0). funct 0x08 = jr $rs; 0x09 = jalr $rd,$rs.
    The transfer target is the value of $rs (bits 25..21) at execution.
    """
    if word >> 26 != 0:
        return None
    funct = word & 0x3F
    rs = (word >> 21) & 0x1F
    if funct == 0x08:  # jr
        return (rs, False)
    if funct == 0x09:  # jalr
        return (rs, True)
    return None


def build_sites(rom: bytes, pcs, va_base: int, rom_base: int):
    """Decode candidate PCs into watch sites.

    `pcs` are code-segment VAs (e.g. from FN64_DISCOVER_PRINT_OPEN); a PC
    that decodes as jr/jalr becomes a site. va_base/rom_base map VA->file
    offset for this bank (MM code segment: va 0x800a5ac0 lives at the
    request_dma_0 VROM — pass its resident file offset).
    """
    sites = []
    for pc in pcs:
        off = rom_base + (pc - va_base)
        if off < 0 or off + 4 > len(rom):
            continue
        dec = decode_indirect(_u32(rom, off))
        if dec:
            src_reg, via_call = dec
            sites.append({"site": f"0x{pc:08x}", "src_reg": src_reg, "via_call": via_call})
    return sites


# --- trace emission (fn64 schema) ---


def normalized_sha256(rom: bytes) -> str:
    # z64 is already big-endian; the discover pipeline hashes the
    # normalized (big-endian) bytes, which for a native .z64 is the file.
    if rom[:4] != b"\x80\x37\x12\x40":
        raise SystemExit("ROM is not native big-endian .z64 (refusing to guess byte order)")
    return hashlib.sha256(rom).hexdigest()


def bank_addr(bank: str, addr: int) -> dict:
    return {"address": addr & 0xFFFFFFFF, "bank": {"status": "known", "bank": bank, "activation": 0}}


def run(args):
    rom = open(args.rom, "rb").read()
    digest = normalized_sha256(rom)
    sites = json.load(open(args.sites))
    if not sites:
        raise SystemExit("no sites to watch")

    # site VA -> (src_reg, via_call)
    by_pc = {}
    for s in sites:
        pc = int(s["site"], 16) if isinstance(s["site"], str) else s["site"]
        by_pc[pc & 0xFFFFFFFF] = (int(s["src_reg"]), bool(s.get("via_call", False)))

    gdb = GdbClient(args.host, args.port)
    out = open(args.out, "w")
    seq = 0

    def emit(rec):
        nonlocal seq
        out.write(json.dumps(rec) + "\n")
        seq += 1

    emit({
        "event": "header", "sequence": 0, "schema_version": 1,
        "normalized_rom_sha256": digest, "trace_id": args.trace_id,
        "producer": "ares-gdb-trace v1 (ares v148 GDB stub, breakpoint-driven jr/jalr capture)",
    })
    seq = 1

    print(f"handshake: {gdb.handshake()!r}", file=sys.stderr)
    for pc in by_pc:
        reply = gdb.set_breakpoint(pc)
        if reply != "OK":
            print(f"WARN breakpoint 0x{pc:08x} -> {reply!r}", file=sys.stderr)
    print(f"installed {len(by_pc)} breakpoints; running for {args.duration}s", file=sys.stderr)

    edges = set()
    deadline = None  # set after we pass the first monotonic read
    start = None
    gdb.cont()
    while True:
        if start is None:
            start = 0.0  # first iteration; wall clock via socket timeouts
        reply = gdb.wait_for_stop(timeout=args.poll)
        if reply.startswith(("T", "S")):
            # Stopped at a breakpoint: read PC and the site's source reg.
            try:
                pc = gdb.read_reg(PC_REG_INDEX) & 0xFFFFFFFF
            except IOError as e:
                print(f"pc read failed: {e}", file=sys.stderr)
                break
            hit = by_pc.get(pc)
            if hit is not None:
                src_reg, via_call = hit
                target = gdb.read_reg(src_reg) & 0xFFFFFFFF
                key = (pc, target)
                if key not in edges:
                    edges.add(key)
                    emit({
                        "event": "indirect_transfer", "sequence": seq,
                        "kind": "call" if via_call else "jump",
                        "site": bank_addr(args.bank, pc),
                        "target": bank_addr(args.bank, target),
                    })
                    print(f"edge 0x{pc:08x} -> 0x{target:08x} ({len(edges)} unique)", file=sys.stderr)
            gdb.cont()  # resume until the next break
        # crude wall-clock budget: each poll is <= args.poll seconds
        args.duration -= args.poll
        if args.duration <= 0:
            break

    emit({"event": "end", "sequence": seq, "completion": "completed", "exhaustiveness": []})
    out.close()
    gdb.close()
    print(f"done: {len(edges)} unique edges -> {args.out}", file=sys.stderr)


def emit_sites(args):
    """Decode a list of candidate PCs into a sites.json (offline, no ares).

    `--pcs` is a comma-separated list of VAs (or @file with one hex VA per
    line, e.g. the `code-seg open [...]` VAs from FN64_DISCOVER_PRINT_OPEN).
    Only PCs that decode as jr/jalr become sites.
    """
    rom = open(args.rom, "rb").read()
    raw = args.pcs
    if raw.startswith("@"):
        raw = " ".join(open(raw[1:]).read().split())
    pcs = [int(tok, 16) for tok in raw.replace(",", " ").split() if tok]
    sites = build_sites(rom, pcs, args.va_base, args.rom_base)
    json.dump(sites, open(args.out, "w"), indent=2)
    print(f"{len(sites)} jr/jalr sites of {len(pcs)} PCs -> {args.out}", file=sys.stderr)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd")

    cap = sub.add_parser("capture", help="drive ares over GDB and emit a trace")
    cap.add_argument("--rom", required=True)
    cap.add_argument("--sites", required=True, help="JSON list of {site, src_reg, via_call}")
    cap.add_argument("--out", required=True)
    cap.add_argument("--trace-id", default="ares-1")
    cap.add_argument("--bank", default="request_dma_0", help="fn64 bank name for site/target")
    cap.add_argument("--host", default="127.0.0.1")
    cap.add_argument("--port", type=int, default=9123)
    cap.add_argument("--duration", type=float, default=120.0, help="capture seconds")
    cap.add_argument("--poll", type=float, default=1.0, help="stop-reply poll interval")

    es = sub.add_parser("emit-sites", help="decode candidate PCs into a sites.json")
    es.add_argument("--rom", required=True)
    es.add_argument("--pcs", required=True, help="comma/space VAs, or @file of hex VAs")
    es.add_argument("--out", required=True)
    es.add_argument("--va-base", type=lambda x: int(x, 0), default=0x800A5AC0,
                    help="bank VA base (MM request_dma_0 = 0x800a5ac0)")
    es.add_argument("--rom-base", type=lambda x: int(x, 0), required=True,
                    help="file offset of --va-base (this bank's resident VROM start)")

    args = ap.parse_args()
    if args.cmd == "emit-sites":
        emit_sites(args)
    elif args.cmd == "capture":
        run(args)
    else:
        ap.print_help()
        raise SystemExit(2)


if __name__ == "__main__":
    main()
