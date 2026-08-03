// Export a loader-neutral function inventory for one exact mapped bank.
// @category fn64

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.address.AddressRange;
import ghidra.program.model.address.AddressSpace;
import ghidra.program.model.address.AddressSetView;
import ghidra.program.model.address.AddressIterator;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionIterator;
import ghidra.program.model.mem.Memory;
import ghidra.program.model.mem.MemoryBlock;
import ghidra.program.model.symbol.SymbolTable;

import java.io.BufferedWriter;
import java.io.FileOutputStream;
import java.io.OutputStreamWriter;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HexFormat;
import java.util.List;

public class Fn64ExportLoaderComparison extends GhidraScript {
    private record BodyRange(long start, long end) {}
    private record FunctionBody(long entry, List<BodyRange> ranges) {}
    private record RejectedFunction(long entry, String reason) {}
    private record FunctionInventory(
        List<FunctionBody> functions,
        List<RejectedFunction> rejectedFunctions
    ) {}
    private record BlockGeometry(
        long start,
        long end,
        long overlapStart,
        long overlapEnd,
        boolean read,
        boolean write,
        boolean execute,
        boolean initialized
    ) {}
    private record MappingInventory(List<BlockGeometry> blocks) {}

    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length != 14) {
            throw new IllegalArgumentException(
                "usage: OUT LANE PHASE BANK VA_START VA_END CONTEXT_START CONTEXT_END " +
                "ROM_SHA BANK_SHA CONTEXT_SHA MAPPING_SHA PROVENANCE_SHA PROGRAM_NAME"
            );
        }

        String output = args[0];
        String lane = args[1];
        if (!lane.equals("binary-loader") && !lane.equals("n64loaderwv")) {
            throw new IllegalArgumentException("lane must be binary-loader or n64loaderwv");
        }
        String phase = args[2];
        if (!phase.equals("pre") && !phase.equals("post")) {
            throw new IllegalArgumentException("phase must be pre or post");
        }
        String bank = requireToken(args[3], "bank");
        long vaStart = parseU32(args[4]);
        long vaEnd = parseU32(args[5]);
        if (vaStart >= vaEnd || (vaStart & 3) != 0 || (vaEnd & 3) != 0) {
            throw new IllegalArgumentException("invalid bank interval");
        }
        long contextStart = parseU32(args[6]);
        long contextEnd = parseU32(args[7]);
        if (contextStart >= contextEnd || (contextStart & 3) != 0 ||
                (contextEnd & 3) != 0 || vaStart < contextStart || vaEnd > contextEnd) {
            throw new IllegalArgumentException("invalid context interval or bank containment");
        }
        String romSha = requireSha(args[8]);
        String bankSha = requireSha(args[9]);
        String contextSha = requireSha(args[10]);
        String mappingSha = requireSha(args[11]);
        String provenanceSha = requireSha(args[12]);
        String expectedProgramName = requireToken(args[13], "program name");
        if (!currentProgram.getName().equals(expectedProgramName)) {
            throw new IllegalStateException("wrong program: " + currentProgram.getName());
        }

        AddressSpace addressSpace = currentProgram.getAddressFactory().getDefaultAddressSpace();
        MappingInventory mapping = verifyMappedContext(
            addressSpace, contextStart, contextEnd, contextSha, vaStart, vaEnd, bankSha
        );
        FunctionInventory functionInventory = collectFunctions(addressSpace, vaStart, vaEnd);
        List<FunctionBody> functions = functionInventory.functions();
        List<RejectedFunction> rejectedFunctions = functionInventory.rejectedFunctions();
        String inventorySha = inventoryDigest(functions);
        String rejectedFunctionsSha = rejectedFunctionsDigest(rejectedFunctions);
        List<Long> entryPoints = collectEntryPoints(addressSpace, vaStart, vaEnd);
        String entryPointsSha = entryPointsDigest(entryPoints);

        try (BufferedWriter writer = new BufferedWriter(new OutputStreamWriter(
                new FileOutputStream(output), StandardCharsets.UTF_8))) {
            writer.write("{\"schema\":\"fn64.ghidra-bank-function-inventory\",\"schema_version\":4");
            writer.write(",\"candidate_only\":true");
            writer.write(",\"provenance\":{\"lane\":\"" + lane +
                "\",\"phase\":\"" + phase + "\",\"source_sha256\":\"" +
                provenanceSha + "\"}");
            writer.write(",\"input\":{\"normalized_rom_sha256\":\"" + romSha +
                "\",\"bank\":\"" + json(bank) + "\",\"bank_bytes_sha256\":\"" +
                bankSha + "\",\"context_bytes_sha256\":\"" + contextSha +
                "\",\"mapping_sha256\":\"" + mappingSha +
                "\",\"va_start\":" + vaStart + ",\"va_end\":" + vaEnd +
                ",\"context_start\":" + contextStart +
                ",\"context_end\":" + contextEnd + "}");
            writer.write(",\"memory_blocks\":[");
            for (int blockIndex = 0; blockIndex < mapping.blocks().size(); blockIndex++) {
                if (blockIndex != 0) {
                    writer.write(",");
                }
                BlockGeometry block = mapping.blocks().get(blockIndex);
                writer.write("{\"va_start\":" + block.start() +
                    ",\"va_end\":" + block.end() +
                    ",\"overlap_start\":" + block.overlapStart() +
                    ",\"overlap_end\":" + block.overlapEnd() +
                    ",\"read\":" + block.read() +
                    ",\"write\":" + block.write() +
                    ",\"execute\":" + block.execute() +
                    ",\"initialized\":" + block.initialized() + "}");
            }
            writer.write("]");
            writer.write(",\"entry_point_count\":" + entryPoints.size() +
                ",\"entry_points_sha256\":\"" + entryPointsSha + "\"");
            writer.write(",\"entry_points\":[");
            for (int entryIndex = 0; entryIndex < entryPoints.size(); entryIndex++) {
                if (entryIndex != 0) {
                    writer.write(",");
                }
                writer.write(Long.toUnsignedString(entryPoints.get(entryIndex)));
            }
            writer.write("]");
            writer.write(",\"rejected_function_count\":" + rejectedFunctions.size() +
                ",\"rejected_functions_sha256\":\"" + rejectedFunctionsSha + "\"");
            writer.write(",\"rejected_functions\":[");
            for (int rejectedIndex = 0; rejectedIndex < rejectedFunctions.size(); rejectedIndex++) {
                if (rejectedIndex != 0) {
                    writer.write(",");
                }
                RejectedFunction rejected = rejectedFunctions.get(rejectedIndex);
                writer.write("{\"entry\":" + rejected.entry() +
                    ",\"reason\":\"" + rejected.reason() + "\"}");
            }
            writer.write("]");
            writer.write(",\"function_count\":" + functions.size() +
                ",\"function_inventory_sha256\":\"" + inventorySha + "\"");
            writer.write(",\"functions\":[");
            for (int functionIndex = 0; functionIndex < functions.size(); functionIndex++) {
                if (functionIndex != 0) {
                    writer.write(",");
                }
                FunctionBody function = functions.get(functionIndex);
                writer.write("{\"entry\":" + function.entry() + ",\"body_ranges\":[");
                for (int rangeIndex = 0; rangeIndex < function.ranges().size(); rangeIndex++) {
                    if (rangeIndex != 0) {
                        writer.write(",");
                    }
                    BodyRange range = function.ranges().get(rangeIndex);
                    writer.write("{\"va_start\":" + range.start() +
                        ",\"va_end\":" + range.end() + "}");
                }
                writer.write("]}");
            }
            writer.write("]}\n");
        }
    }

    private MappingInventory verifyMappedContext(
            AddressSpace addressSpace,
            long contextStart,
            long contextEnd,
            String expectedContextSha,
            long bankStart,
            long bankEnd,
            String expectedBankSha)
            throws Exception {
        long length = Math.subtractExact(contextEnd, contextStart);
        if (length <= 0 || length > Integer.MAX_VALUE) {
            throw new IllegalStateException("context interval length is unsupported or overflowed");
        }

        Address start = addressSpace.getAddress(contextStart);
        Address end = addressSpace.getAddress(contextEnd - 1);
        if (!start.getAddressSpace().equals(addressSpace) ||
                !end.getAddressSpace().equals(addressSpace) ||
                start.getUnsignedOffset() != contextStart ||
                end.getUnsignedOffset() != contextEnd - 1) {
            throw new IllegalStateException("context interval is not in the default address space");
        }

        Memory memory = currentProgram.getMemory();
        MessageDigest contextDigest = MessageDigest.getInstance("SHA-256");
        MessageDigest bankDigest = MessageDigest.getInstance("SHA-256");
        List<BlockGeometry> blocks = new ArrayList<>();
        byte[] buffer = new byte[64 * 1024];
        long consumed = 0;
        long bankConsumed = 0;
        long previousBlockStart = -1;
        long previousBlockEnd = -1;
        while (consumed < length) {
            Address chunkStart = start.addNoWrap(consumed);
            MemoryBlock block = memory.getBlock(chunkStart);
            if (block == null) {
                throw new IllegalStateException("context interval contains unmapped memory at " + chunkStart);
            }
            if (!block.getStart().getAddressSpace().equals(addressSpace) || block.isOverlay()) {
                throw new IllegalStateException(
                    "context interval resolves through a non-default address space at " + chunkStart
                );
            }
            if (!block.isRead()) {
                throw new IllegalStateException("context interval is not readable at " + chunkStart);
            }
            long blockStart = block.getStart().getUnsignedOffset();
            long blockMax = block.getEnd().getUnsignedOffset();
            if (blockMax > 0xffff_ffffL || blockStart > blockMax) {
                throw new IllegalStateException("context memory block has an invalid u32 extent");
            }
            long blockEnd = blockMax + 1;
            if (blockStart != previousBlockStart || blockEnd != previousBlockEnd) {
                blocks.add(new BlockGeometry(
                    blockStart,
                    blockEnd,
                    Math.max(blockStart, contextStart),
                    Math.min(blockEnd, contextEnd),
                    block.isRead(),
                    block.isWrite(),
                    block.isExecute(),
                    block.isInitialized()
                ));
                previousBlockStart = blockStart;
                previousBlockEnd = blockEnd;
            }
            long bytesInBlock = Math.subtractExact(blockEnd, chunkStart.getUnsignedOffset());
            int chunkLength = (int) Math.min(
                buffer.length, Math.min(length - consumed, bytesInBlock)
            );
            if (chunkLength <= 0) {
                throw new IllegalStateException("context memory block has an invalid extent at " + chunkStart);
            }
            int bytesRead = memory.getBytes(chunkStart, buffer, 0, chunkLength);
            if (bytesRead != chunkLength) {
                throw new IllegalStateException("context interval became unreadable at " + chunkStart);
            }
            contextDigest.update(buffer, 0, chunkLength);
            long chunkOffset = chunkStart.getUnsignedOffset();
            long chunkEnd = Math.addExact(chunkOffset, chunkLength);
            long bankOverlapStart = Math.max(chunkOffset, bankStart);
            long bankOverlapEnd = Math.min(chunkEnd, bankEnd);
            if (bankOverlapStart < bankOverlapEnd) {
                int bufferOffset = (int) (bankOverlapStart - chunkOffset);
                int bankLength = (int) (bankOverlapEnd - bankOverlapStart);
                bankDigest.update(buffer, bufferOffset, bankLength);
                bankConsumed = Math.addExact(bankConsumed, bankLength);
            }
            consumed = Math.addExact(consumed, chunkLength);
        }
        if (bankConsumed != bankEnd - bankStart) {
            throw new IllegalStateException("bank interval was not fully covered by the context");
        }
        String actualContextSha = HexFormat.of().formatHex(contextDigest.digest());
        if (!actualContextSha.equals(expectedContextSha)) {
            throw new IllegalStateException(
                "mapped context digest mismatch: expected " + expectedContextSha +
                ", got " + actualContextSha
            );
        }
        String actualBankSha = HexFormat.of().formatHex(bankDigest.digest());
        if (!actualBankSha.equals(expectedBankSha)) {
            throw new IllegalStateException(
                "mapped bank digest mismatch: expected " + expectedBankSha +
                ", got " + actualBankSha
            );
        }
        blocks.sort(Comparator.comparingLong(BlockGeometry::start)
            .thenComparingLong(BlockGeometry::end)
            .thenComparingLong(BlockGeometry::overlapStart)
            .thenComparingLong(BlockGeometry::overlapEnd));
        return new MappingInventory(List.copyOf(blocks));
    }

    private FunctionInventory collectFunctions(
            AddressSpace addressSpace, long vaStart, long vaEnd) {
        List<FunctionBody> result = new ArrayList<>();
        List<RejectedFunction> rejected = new ArrayList<>();
        FunctionIterator functions = currentProgram.getFunctionManager().getFunctions(true);
        for (Function function : functions) {
            if (function.isExternal()) {
                continue;
            }
            Address entryPoint = function.getEntryPoint();
            long entry = entryPoint.getUnsignedOffset();
            if (entry < vaStart || entry >= vaEnd) {
                continue;
            }
            if (!entryPoint.getAddressSpace().equals(addressSpace)) {
                throw new IllegalStateException(
                    "function entry in bank uses a non-default address space at " + entryPoint
                );
            }
            if ((entry & 3) != 0) {
                throw new IllegalStateException("function entry is not word-aligned at " + entryPoint);
            }

            AddressSetView body = function.getBody();
            long expectedRangeCount = body.getNumAddressRanges();
            if (expectedRangeCount <= 0 || expectedRangeCount > Integer.MAX_VALUE) {
                throw new IllegalStateException("function body has an invalid range count at " + entryPoint);
            }
            List<BodyRange> ranges = new ArrayList<>((int) expectedRangeCount);
            boolean containsEntry = false;
            boolean rejectedBody = false;
            for (AddressRange range : body.getAddressRanges(true)) {
                Address rangeMin = range.getMinAddress();
                Address rangeMax = range.getMaxAddress();
                if (!rangeMin.getAddressSpace().equals(addressSpace) ||
                        !rangeMax.getAddressSpace().equals(addressSpace)) {
                    throw new IllegalStateException("function body crosses address spaces at " + entryPoint);
                }
                long bodyStart = rangeMin.getUnsignedOffset();
                long bodyMax = rangeMax.getUnsignedOffset();
                if (bodyMax >= 0xffff_ffffL) {
                    throw new IllegalStateException("function body range end overflows u32 at " + entryPoint);
                }
                long bodyEnd = bodyMax + 1;
                if (bodyStart < vaStart || bodyEnd <= bodyStart || bodyEnd > vaEnd) {
                    throw new IllegalStateException(
                        "function body violates the supplied bank mapping at " + entryPoint +
                        ": [0x" + Long.toHexString(bodyStart) + ",0x" +
                        Long.toHexString(bodyEnd) + ") outside [0x" +
                        Long.toHexString(vaStart) + ",0x" + Long.toHexString(vaEnd) + ")"
                    );
                }
                if ((bodyStart & 3) != 0 || (bodyEnd & 3) != 0) {
                    rejected.add(new RejectedFunction(entry, "non_word_body_range"));
                    rejectedBody = true;
                    break;
                }
                containsEntry |= bodyStart <= entry && entry < bodyEnd;
                ranges.add(new BodyRange(bodyStart, bodyEnd));
            }
            if (rejectedBody) {
                continue;
            }
            if (ranges.size() != expectedRangeCount) {
                throw new IllegalStateException("function body range count changed at " + entryPoint);
            }
            ranges.sort(Comparator.comparingLong(BodyRange::start).thenComparingLong(BodyRange::end));
            for (int index = 1; index < ranges.size(); index++) {
                if (ranges.get(index - 1).end() > ranges.get(index).start()) {
                    throw new IllegalStateException("function body ranges overlap at " + entryPoint);
                }
            }
            if (!containsEntry) {
                throw new IllegalStateException("function body does not contain its entry at " + entryPoint);
            }
            result.add(new FunctionBody(entry, List.copyOf(ranges)));
        }

        result.sort(Comparator.comparingLong(FunctionBody::entry));
        for (int index = 1; index < result.size(); index++) {
            if (result.get(index - 1).entry() == result.get(index).entry()) {
                throw new IllegalStateException("duplicate function entry in bank");
            }
        }
        rejected.sort(Comparator.comparingLong(RejectedFunction::entry));
        for (int index = 1; index < rejected.size(); index++) {
            if (rejected.get(index - 1).entry() == rejected.get(index).entry()) {
                throw new IllegalStateException("duplicate rejected function entry in bank");
            }
        }
        return new FunctionInventory(List.copyOf(result), List.copyOf(rejected));
    }

    private List<Long> collectEntryPoints(
            AddressSpace addressSpace, long vaStart, long vaEnd) {
        SymbolTable symbolTable = currentProgram.getSymbolTable();
        AddressIterator iterator = symbolTable.getExternalEntryPointIterator();
        List<Long> result = new ArrayList<>();
        while (iterator.hasNext()) {
            Address address = iterator.next();
            long offset = address.getUnsignedOffset();
            if (offset < vaStart || offset >= vaEnd) {
                continue;
            }
            if (!address.getAddressSpace().equals(addressSpace)) {
                throw new IllegalStateException(
                    "entry point in bank uses a non-default address space at " + address
                );
            }
            if ((offset & 3) != 0) {
                throw new IllegalStateException("entry point is not word-aligned at " + address);
            }
            result.add(offset);
        }
        result.sort(Long::compareUnsigned);
        for (int index = 1; index < result.size(); index++) {
            if (result.get(index - 1).equals(result.get(index))) {
                throw new IllegalStateException("duplicate entry point in bank");
            }
        }
        return List.copyOf(result);
    }

    private static String inventoryDigest(List<FunctionBody> functions) throws Exception {
        MessageDigest digest = MessageDigest.getInstance("SHA-256");
        digest.update("fn64.ghidra-bank-function-inventory.v1\0".getBytes(StandardCharsets.UTF_8));
        putU64(digest, functions.size());
        for (FunctionBody function : functions) {
            putU32(digest, function.entry());
            putU64(digest, function.ranges().size());
            for (BodyRange range : function.ranges()) {
                putU32(digest, range.start());
                putU32(digest, range.end());
            }
        }
        return HexFormat.of().formatHex(digest.digest());
    }

    private static String entryPointsDigest(List<Long> entryPoints) throws Exception {
        MessageDigest digest = MessageDigest.getInstance("SHA-256");
        digest.update("fn64.ghidra-bank-entry-points.v1\0".getBytes(StandardCharsets.UTF_8));
        putU64(digest, entryPoints.size());
        for (long entryPoint : entryPoints) {
            putU32(digest, entryPoint);
        }
        return HexFormat.of().formatHex(digest.digest());
    }

    private static String rejectedFunctionsDigest(List<RejectedFunction> rejected)
            throws Exception {
        MessageDigest digest = MessageDigest.getInstance("SHA-256");
        digest.update("fn64.ghidra-bank-rejected-functions.v1\0".getBytes(StandardCharsets.UTF_8));
        putU64(digest, rejected.size());
        for (RejectedFunction function : rejected) {
            putU32(digest, function.entry());
            byte[] reason = function.reason().getBytes(StandardCharsets.UTF_8);
            putU64(digest, reason.length);
            digest.update(reason);
        }
        return HexFormat.of().formatHex(digest.digest());
    }

    private static void putU64(MessageDigest digest, long value) {
        digest.update(ByteBuffer.allocate(8).order(ByteOrder.LITTLE_ENDIAN).putLong(value).array());
    }

    private static void putU32(MessageDigest digest, long value) {
        digest.update(ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt((int) value).array());
    }

    private static long parseU32(String value) {
        long parsed = Long.decode(value);
        if (parsed < 0 || parsed > 0xffff_ffffL) {
            throw new IllegalArgumentException("not a u32: " + value);
        }
        return parsed;
    }

    private static String requireSha(String value) {
        if (!value.matches("[0-9a-f]{64}")) {
            throw new IllegalArgumentException("digest must be lowercase SHA-256");
        }
        return value;
    }

    private static String requireToken(String value, String label) {
        if (value.isEmpty() || value.length() > 128 ||
                value.chars().anyMatch(Character::isISOControl)) {
            throw new IllegalArgumentException("invalid " + label);
        }
        return value;
    }

    private static String json(String value) {
        return value.replace("\\", "\\\\").replace("\"", "\\\"");
    }
}
