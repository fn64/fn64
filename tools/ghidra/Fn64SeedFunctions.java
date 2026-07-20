// Seed a bank-local Ghidra analysis without giving Ghidra ownership of mappings.
// @category fn64

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;

public class Fn64SeedFunctions extends GhidraScript {
    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length < 3) {
            throw new IllegalArgumentException("usage: MODE VA_START VA_END [SEED ...]");
        }

        String mode = args[0];
        if (!mode.equals("unseeded") && !mode.equals("seeded")) {
            throw new IllegalArgumentException("mode must be unseeded or seeded");
        }

        long vaStart = parseU32(args[1]);
        long vaEnd = parseU32(args[2]);
        int requiredSeeds = mode.equals("unseeded") ? 1 : 2;
        if (args.length != 3 + requiredSeeds) {
            throw new IllegalArgumentException(
                mode + " mode requires exactly " + requiredSeeds + " seed addresses"
            );
        }

        for (int index = 3; index < args.length; index++) {
            long seed = parseU32(args[index]);
            if (seed < vaStart || seed >= vaEnd || (seed & 3) != 0) {
                throw new IllegalArgumentException("seed is outside or unaligned in the bank");
            }
            Address address = toAddr(seed);
            if (!disassemble(address)) {
                throw new IllegalStateException("failed to disassemble seed " + args[index]);
            }
            if (getFunctionAt(address) == null && createFunction(address, null) == null) {
                throw new IllegalStateException("failed to create function at " + args[index]);
            }
        }
    }

    private static long parseU32(String value) {
        long parsed = Long.decode(value);
        if (parsed < 0 || parsed > 0xffff_ffffL) {
            throw new IllegalArgumentException("not a u32: " + value);
        }
        return parsed;
    }
}
