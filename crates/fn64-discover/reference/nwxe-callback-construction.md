# NWXE callerless functions with proven address construction

The six NWXE answer functions that no `jal` reaches anywhere in the 32 MB
ROM, but whose addresses ARE assembled in code from split `lui`+`addiu`/`ori`
immediate pairs. A 32-bit pointer scan cannot see these; only immediate
tracking can.

Each row names the function that builds the address and the exact site, so a
mechanical verifier has a specific claim to confirm and a trace run has a
specific place to look.

| callback | constructed inside | site VA | note |
|---|---|---|---|
| `0x80000460` | `func_80000400` (ROM entry point) | `0x8000042c` | boot path installs it |
| `0x80028ba0` | `func_80025FE8` | `0x80026108` | two sites |
| `0x80028ba0` | `func_80029BD0` | `0x80029cf0` | |
| `0x8002c290` | `func_8002C054` | `0x8002c074` | |
| `0x8002c290` | `func_8002CA70` | `0x8002cac8` | next insn stores it to memory |
| `0x8002ceb0` | `func_8002BF9C` | `0x8002bfb4` | |
| `0x8002db80` | `func_8002C1EC` | `0x8002c20c` | |
| `0x8002dfa0` | `func_8002BB04` | `0x8002bb54` | next insn stores it to memory |

Shape evidence that these are a real API rather than linker residue: bodies
run 0x140-0x500 bytes, save six to eight callee-saved registers, and read
arguments past `$a3` off the incoming stack (`lw $8,104($sp)`). Two
constructions are immediately followed by `sw`, which is a callback being
installed into a struct or table.

`func_8002DB80` also builds `0x00200440` and `0x00200580` by `lui`+`ori`;
those are not code addresses and read as DMA lengths or DMEM offsets, which
would place it in a hardware or microcode lane.

**Status: hypothesis, not evidence.** Address construction proves the value
can exist at runtime; it does not prove any particular call happens. These
are the highest-confidence targets for the trace lane (M4) precisely because
the static side has taken them as far as it can.
