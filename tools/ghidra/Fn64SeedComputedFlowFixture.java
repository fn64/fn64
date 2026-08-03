// Seed the handwritten computed-control-flow conformance fixture.
// @category fn64

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;

public class Fn64SeedComputedFlowFixture extends GhidraScript {
    @Override
    protected void run() throws Exception {
        long[] functionStarts = {
            0x80001000L, 0x80001020L, 0x80001040L, 0x80001060L,
            0x80001120L, 0x80001160L, 0x80001180L, 0x800011a0L
        };
        for (long value : functionStarts) {
            Address address = toAddr(value);
            if (!disassemble(address)) {
                throw new IllegalStateException("disassembly failed at " + address);
            }
            if (getFunctionAt(address) == null && createFunction(address, null) == null) {
                throw new IllegalStateException("function creation failed at " + address);
            }
        }
    }
}
