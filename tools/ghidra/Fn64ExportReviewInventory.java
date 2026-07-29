// Export a path-free, candidate-only inventory for N64LoaderWV first-contact review.
// @category fn64

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.address.AddressRange;
import ghidra.program.model.address.AddressSetView;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionIterator;
import ghidra.program.model.mem.MemoryBlock;

import java.io.BufferedWriter;
import java.io.FileOutputStream;
import java.io.OutputStreamWriter;
import java.nio.charset.StandardCharsets;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;
import java.util.Set;

public class Fn64ExportReviewInventory extends GhidraScript {
    private static final String SCHEMA = "fn64.n64loaderwv-review-inventory.v2";
    private static final String ZERO_SHA256 =
        "0000000000000000000000000000000000000000000000000000000000000000";

    private record BlockInventory(
        String name,
        String addressSpace,
        long start,
        long end,
        long size,
        boolean read,
        boolean write,
        boolean execute,
        boolean overlay
    ) {}

    private record BodyRange(long start, long endExclusive) {}

    private record FunctionInventory(
        long entry,
        long bodyEnvelopeStart,
        long bodyEnvelopeEndExclusive,
        List<BodyRange> bodyRanges,
        String name,
        String source,
        String block,
        String addressSpace,
        boolean reachableFromLoaderEntry
    ) {}

    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length != 10) {
            throw new IllegalArgumentException(
                "usage: OUT ROM_SHA RDRAM_SHA GHIDRA_VERSION LOADER_REPOSITORY " +
                "LOADER_COMMIT EXTENSION_SHA BUILD_RECEIPT_SHA CONFIG_SHA PROGRAM_NAME"
            );
        }

        String output = args[0];
        String romSha = requireSha(args[1], "ROM digest");
        String rdramSha = requireSha(args[2], "RDRAM digest");
        String ghidraVersion = requireToken(args[3], "Ghidra version");
        String loaderRepository = requireToken(args[4], "loader repository");
        String loaderCommit = requireCommit(args[5]);
        String extensionSha = requireSha(args[6], "extension digest");
        String buildReceiptSha = requireSha(args[7], "build receipt digest");
        String configSha = requireSha(args[8], "configuration digest");
        String expectedProgramName = requireToken(args[9], "program name");
        if (!currentProgram.getName().equals(expectedProgramName)) {
            throw new IllegalStateException("wrong program: " + currentProgram.getName());
        }

        List<BlockInventory> blocks = collectBlocks();
        List<FunctionInventory> functions = collectFunctions();

        try (BufferedWriter writer = new BufferedWriter(new OutputStreamWriter(
                new FileOutputStream(output), StandardCharsets.UTF_8))) {
            writer.write("{\"schema\":\"" + SCHEMA + "\",\"candidate_only\":true");
            writer.write(",\"program_name\":\"" + json(currentProgram.getName()) + "\"");
            writer.write(",\"provenance\":{\"rom_sha256\":\"" + romSha +
                "\",\"rdram_sha256\":\"" + rdramSha + "\",\"rdram_present\":" +
                !rdramSha.equals(ZERO_SHA256) + ",\"ghidra_version\":\"" +
                json(ghidraVersion) + "\",\"loader_repository\":\"" +
                json(loaderRepository) + "\",\"loader_commit\":\"" + loaderCommit +
                "\",\"extension_sha256\":\"" + extensionSha +
                "\",\"build_receipt_sha256\":\"" + buildReceiptSha +
                "\",\"config_sha256\":\"" + configSha + "\"}");
            long reachableCount = functions.stream()
                .filter(FunctionInventory::reachableFromLoaderEntry)
                .count();
            writer.write(",\"counts\":{\"memory_blocks\":" + blocks.size() +
                ",\"functions\":" + functions.size() +
                ",\"reachable_from_loader_entries\":" + reachableCount + "}");

            writer.write(",\"memory_blocks\":[");
            for (int index = 0; index < blocks.size(); index++) {
                if (index != 0) {
                    writer.write(",");
                }
                writeBlock(writer, blocks.get(index));
            }
            writer.write("]");

            writer.write(",\"functions\":[");
            for (int index = 0; index < functions.size(); index++) {
                if (index != 0) {
                    writer.write(",");
                }
                writeFunction(writer, functions.get(index));
            }
            writer.write("]}\n");
        }
    }

    private List<BlockInventory> collectBlocks() {
        List<BlockInventory> result = new ArrayList<>();
        for (MemoryBlock block : currentProgram.getMemory().getBlocks()) {
            if (block.isExternalBlock()) {
                continue;
            }
            Address start = block.getStart();
            Address end = block.getEnd();
            if (!start.getAddressSpace().equals(end.getAddressSpace())) {
                throw new IllegalStateException("memory block crosses address spaces: " + block.getName());
            }
            result.add(new BlockInventory(
                block.getName(),
                start.getAddressSpace().getName(),
                start.getUnsignedOffset(),
                end.getUnsignedOffset(),
                block.getSize(),
                block.isRead(),
                block.isWrite(),
                block.isExecute(),
                block.isOverlay()
            ));
        }
        result.sort(Comparator.comparing(BlockInventory::addressSpace)
            .thenComparing(BlockInventory::start, Long::compareUnsigned)
            .thenComparing(BlockInventory::end, Long::compareUnsigned)
            .thenComparing(BlockInventory::name));
        return result;
    }

    private List<FunctionInventory> collectFunctions() throws Exception {
        List<FunctionInventory> result = new ArrayList<>();
        Set<Address> reachable = reachableFromLoaderEntries();
        FunctionIterator functions = currentProgram.getFunctionManager().getFunctions(true);
        for (Function function : functions) {
            if (function.isExternal()) {
                continue;
            }

            AddressSetView body = function.getBody();
            if (body.isEmpty()) {
                throw new IllegalStateException("function has an empty body at " + function.getEntryPoint());
            }
            Address entry = function.getEntryPoint();
            MemoryBlock block = currentProgram.getMemory().getBlock(entry);
            if (block == null || block.isExternalBlock()) {
                throw new IllegalStateException(
                    "non-external function is not in loader memory at " + function.getEntryPoint()
                );
            }
            List<BodyRange> bodyRanges = new ArrayList<>();
            for (AddressRange range : body.getAddressRanges(true)) {
                Address rangeMin = range.getMinAddress();
                Address rangeMax = range.getMaxAddress();
                if (!entry.getAddressSpace().equals(rangeMin.getAddressSpace()) ||
                        !entry.getAddressSpace().equals(rangeMax.getAddressSpace())) {
                    throw new IllegalStateException(
                        "function body crosses address spaces at " + function.getEntryPoint()
                    );
                }
                if (!currentProgram.getMemory().contains(rangeMin, rangeMax)) {
                    throw new IllegalStateException(
                        "function body contains unmapped addresses at " + function.getEntryPoint()
                    );
                }
                if (!block.contains(rangeMin) || !block.contains(rangeMax)) {
                    throw new IllegalStateException(
                        "function body spans loader memory blocks at " + function.getEntryPoint()
                    );
                }
                long rangeMaxOffset = rangeMax.getUnsignedOffset();
                if (Long.compareUnsigned(rangeMaxOffset, 0xffff_ffffL) >= 0) {
                    throw new IllegalStateException(
                        "function body range end overflows u32 at " + function.getEntryPoint()
                    );
                }
                long rangeStart = rangeMin.getUnsignedOffset();
                long rangeEndExclusive = rangeMaxOffset + 1;
                if (Long.compareUnsigned(rangeEndExclusive, rangeStart) <= 0) {
                    throw new IllegalStateException(
                        "function body has an overflowing range at " + function.getEntryPoint()
                    );
                }
                bodyRanges.add(new BodyRange(rangeStart, rangeEndExclusive));
            }
            bodyRanges.sort(Comparator.comparing(BodyRange::start, Long::compareUnsigned)
                .thenComparing(BodyRange::endExclusive, Long::compareUnsigned));
            if (bodyRanges.size() != body.getNumAddressRanges()) {
                throw new IllegalStateException(
                    "function body range count changed at " + function.getEntryPoint()
                );
            }
            long bodyEnvelopeStart = bodyRanges.get(0).start();
            long bodyEnvelopeEndExclusive = bodyRanges.get(bodyRanges.size() - 1).endExclusive();

            result.add(new FunctionInventory(
                entry.getUnsignedOffset(),
                bodyEnvelopeStart,
                bodyEnvelopeEndExclusive,
                List.copyOf(bodyRanges),
                function.getName(),
                function.getSymbol().getSource().name().toLowerCase(Locale.ROOT),
                block.getName(),
                entry.getAddressSpace().getName(),
                reachable.contains(entry)
            ));
        }
        result.sort(Comparator.comparing(FunctionInventory::addressSpace)
            .thenComparing(FunctionInventory::entry, Long::compareUnsigned)
            .thenComparing(FunctionInventory::bodyEnvelopeStart, Long::compareUnsigned)
            .thenComparing(FunctionInventory::bodyEnvelopeEndExclusive, Long::compareUnsigned)
            .thenComparing(FunctionInventory::name));
        return result;
    }

    private Set<Address> reachableFromLoaderEntries() throws Exception {
        Set<Address> reachable = new HashSet<>();
        ArrayDeque<Function> pending = new ArrayDeque<>();
        FunctionIterator functions = currentProgram.getFunctionManager().getFunctions(true);
        while (functions.hasNext()) {
            Function function = functions.next();
            if (function.isExternal()) {
                continue;
            }
            String name = function.getName();
            if (name.equals("ramMain") || name.equals("bootMain") || name.equals("pifMain")) {
                pending.add(function);
            }
        }
        while (!pending.isEmpty()) {
            Function function = pending.removeFirst();
            Address entry = function.getEntryPoint();
            if (!reachable.add(entry)) {
                continue;
            }
            for (Function called : function.getCalledFunctions(monitor)) {
                if (!called.isExternal()) {
                    pending.addLast(called);
                }
            }
        }
        return reachable;
    }

    private static void writeBlock(BufferedWriter writer, BlockInventory block) throws Exception {
        writer.write("{\"name\":\"" + json(block.name()) + "\",\"address_space\":\"" +
            json(block.addressSpace()) + "\",\"start\":" + unsigned(block.start()) +
            ",\"end\":" + unsigned(block.end()) + ",\"size\":" + block.size() +
            ",\"read\":" + block.read() + ",\"write\":" + block.write() +
            ",\"execute\":" + block.execute() + ",\"overlay\":" + block.overlay() + "}");
    }

    private static void writeFunction(BufferedWriter writer, FunctionInventory function)
            throws Exception {
        writer.write("{\"entry\":" + unsigned(function.entry()) +
            ",\"body_envelope_start\":" + unsigned(function.bodyEnvelopeStart()) +
            ",\"body_envelope_end_exclusive\":" +
            unsigned(function.bodyEnvelopeEndExclusive()) + ",\"body_ranges\":[");
        for (int index = 0; index < function.bodyRanges().size(); index++) {
            if (index != 0) {
                writer.write(",");
            }
            BodyRange range = function.bodyRanges().get(index);
            writer.write("{\"start\":" + unsigned(range.start()) +
                ",\"end_exclusive\":" + unsigned(range.endExclusive()) + "}");
        }
        writer.write("],\"name\":\"" + json(function.name()) + "\",\"source\":\"" +
            json(function.source()) + "\",\"block\":\"" + json(function.block()) +
            "\",\"address_space\":\"" + json(function.addressSpace()) +
            "\",\"reachable_from_loader_entry\":" + function.reachableFromLoaderEntry() + "}");
    }

    private static String unsigned(long value) {
        return Long.toUnsignedString(value);
    }

    private static String requireSha(String value, String label) {
        if (!value.matches("[0-9a-f]{64}")) {
            throw new IllegalArgumentException(label + " must be lowercase SHA-256");
        }
        return value;
    }

    private static String requireCommit(String value) {
        if (!value.matches("[0-9a-f]{40}")) {
            throw new IllegalArgumentException("loader commit must be a lowercase 40-hex object ID");
        }
        return value;
    }

    private static String requireToken(String value, String label) {
        if (value.isEmpty() || value.length() > 128 || value.chars().anyMatch(Character::isISOControl)) {
            throw new IllegalArgumentException("invalid " + label);
        }
        return value;
    }

    private static String json(String value) {
        StringBuilder escaped = new StringBuilder(value.length());
        for (int index = 0; index < value.length(); index++) {
            char character = value.charAt(index);
            switch (character) {
                case '\"' -> escaped.append("\\\"");
                case '\\' -> escaped.append("\\\\");
                case '\b' -> escaped.append("\\b");
                case '\f' -> escaped.append("\\f");
                case '\n' -> escaped.append("\\n");
                case '\r' -> escaped.append("\\r");
                case '\t' -> escaped.append("\\t");
                default -> {
                    if (character < 0x20) {
                        escaped.append(String.format("\\u%04x", (int) character));
                    }
                    else {
                        escaped.append(character);
                    }
                }
            }
        }
        return escaped.toString();
    }
}
