// Export Ghidra function candidates through fn64's strict candidate-only JSONL schema.
// @category fn64

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.address.AddressRange;
import ghidra.program.model.address.AddressSpace;
import ghidra.program.model.address.AddressSetView;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionIterator;
import ghidra.program.model.mem.Memory;
import ghidra.program.model.mem.MemoryBlock;

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

public class Fn64ExportCandidates extends GhidraScript {
    private record Candidate(int tag, long entry, long start, long end, String providerId) {}
    private record SkippedRange(long start, long end) {}
    private List<SkippedRange> skippedRanges;
    private List<Long> skippedBodyEntries;

    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length != 15) {
            throw new IllegalArgumentException(
                "usage: OUT MODE BANK VA_START VA_END ROM_SHA BANK_SHA MAPPING_SHA " +
                "GHIDRA_VERSION BUILD_SHA CONFIG_SHA EVIDENCE_SHA PROGRAM_NAME " +
                "SNAPSHOT_ROLE SNAPSHOT_SHA"
            );
        }

        String output = args[0];
        String mode = args[1];
        String bank = requireToken(args[2], "bank");
        long vaStart = parseU32(args[3]);
        long vaEnd = parseU32(args[4]);
        if (vaStart >= vaEnd || (vaStart & 3) != 0 || (vaEnd & 3) != 0) {
            throw new IllegalArgumentException("invalid bank interval");
        }
        String romSha = requireSha(args[5]);
        String bankSha = requireSha(args[6]);
        String mappingSha = requireSha(args[7]);
        String ghidraVersion = requireToken(args[8], "Ghidra version");
        String buildSha = requireSha(args[9]);
        String configSha = requireSha(args[10]);
        String evidenceSha = requireSha(args[11]);
        String expectedProgramName = requireToken(args[12], "program name");
        if (!currentProgram.getName().equals(expectedProgramName)) {
            throw new IllegalStateException("wrong program: " + currentProgram.getName());
        }
        if (!mode.equals("unseeded") && !mode.equals("seeded")) {
            throw new IllegalArgumentException("mode must be unseeded or seeded");
        }
        String snapshotRole = args[13];
        if (!snapshotRole.equals("discovery_snapshot")) {
            throw new IllegalArgumentException("lineage role must be discovery_snapshot");
        }
        String snapshotSha = requireSha(args[14]);

        AddressSpace defaultAddressSpace =
            currentProgram.getAddressFactory().getDefaultAddressSpace();
        verifyMappedBank(defaultAddressSpace, vaStart, vaEnd, bankSha);
        skippedRanges = new ArrayList<>();
        skippedBodyEntries = new ArrayList<>();
        List<Candidate> candidates = collectCandidates(bank, vaStart, vaEnd);
        if ("1".equals(System.getenv("FN64_GHIDRA_EXECUTABLE_RANGES"))) {
            candidates = collectExecutableRanges(bank, vaStart, vaEnd);
        }
        String claimsSha = claimsDigest(bank, candidates);
        String toolName = "ghidra-headless-" + mode;

        try (BufferedWriter writer = new BufferedWriter(new OutputStreamWriter(
                new FileOutputStream(output), StandardCharsets.UTF_8))) {
            writer.write("{\"record\":\"header\",\"schema\":\"fn64.tool-adapter\",\"schema_version\":2");
            writer.write(",\"tool\":{\"name\":\"" + toolName + "\",\"version\":\"" +
                json(ghidraVersion) + "\",\"build_sha256\":\"" + buildSha + "\"}");
            writer.write(",\"role\":\"" +
                ("1".equals(System.getenv("FN64_GHIDRA_EXECUTABLE_RANGES"))
                    ? "region_candidates" : "function_boundary_candidates") + "\"");
            writer.write(",\"input\":{\"normalized_rom_sha256\":\"" + romSha +
                "\",\"bank\":\"" + json(bank) + "\",\"bank_bytes_sha256\":\"" + bankSha +
                "\",\"mapping_sha256\":\"" + mappingSha + "\",\"va_start\":" + vaStart +
                ",\"va_end\":" + vaEnd + "}");
            writer.write(",\"lineage\":[{\"role\":\"tool_configuration\",\"source_sha256\":\"" +
                configSha + "\"},{\"role\":\"evidence_manifest\",\"source_sha256\":\"" +
                evidenceSha + "\"},{\"role\":\"" + snapshotRole +
                "\",\"source_sha256\":\"" + snapshotSha + "\"}");
            writer.write("]}\n");

            for (int sequence = 0; sequence < candidates.size(); sequence++) {
                Candidate candidate = candidates.get(sequence);
                writer.write("{\"record\":\"claim\",\"sequence\":" + sequence +
                    ",\"provider_claim_id\":\"" + json(candidate.providerId()) + "\",\"claim\":");
                if (candidate.tag() == 1) {
                    writer.write("{\"type\":\"function_entry\",\"address\":{\"bank\":\"" +
                        json(bank) + "\",\"pc\":" + candidate.start() + "}}");
                } else if (candidate.tag() == 2) {
                    writer.write("{\"type\":\"function_extent\",\"range\":{\"bank\":\"" +
                        json(bank) + "\",\"va_start\":" + candidate.start() +
                        ",\"va_end\":" + candidate.end() + "}}");
                } else if (candidate.tag() == 3) {
                    writer.write("{\"type\":\"executable_range\",\"range\":{\"bank\":\"" +
                        json(bank) + "\",\"va_start\":" + candidate.start() +
                        ",\"va_end\":" + candidate.end() + "}}");
                } else {
                    writer.write("{\"type\":\"function_body_range\",\"entry\":{\"bank\":\"" +
                        json(bank) + "\",\"pc\":" + candidate.entry() +
                        "},\"range\":{\"bank\":\"" + json(bank) + "\",\"va_start\":" +
                        candidate.start() + ",\"va_end\":" + candidate.end() + "}}");
                }
                writer.write("}\n");
            }

            writer.write("{\"record\":\"summary\",\"complete\":true,\"analyzed_range\":{\"bank\":\"" +
                json(bank) + "\",\"va_start\":" + vaStart + ",\"va_end\":" + vaEnd +
                "},\"skipped_ranges\":" + serializeSkippedRanges(bank) +
                ",\"claim_records\":" + candidates.size() +
                ",\"claims_sha256\":\"" + claimsSha + "\",\"resources\":{\"input_bytes\":" +
                (vaEnd - vaStart) + ",\"elapsed_millis\":0,\"peak_memory_bytes\":null," +
                "\"limit_hit\":false,\"warnings\":" + serializeSkippedBodyWarnings() + "}}\n");
        }
    }

    private void verifyMappedBank(
            AddressSpace defaultAddressSpace, long vaStart, long vaEnd, String expectedSha)
            throws Exception {
        long length = Math.subtractExact(vaEnd, vaStart);
        if (length <= 0 || length > Integer.MAX_VALUE) {
            throw new IllegalStateException("bank interval length is unsupported or overflowed");
        }

        Address start = defaultAddressSpace.getAddress(vaStart);
        Address end = defaultAddressSpace.getAddress(vaEnd - 1);
        if (!start.getAddressSpace().equals(defaultAddressSpace) ||
                !end.getAddressSpace().equals(defaultAddressSpace) ||
                start.getUnsignedOffset() != vaStart || end.getUnsignedOffset() != vaEnd - 1) {
            throw new IllegalStateException("bank interval is not in the default address space");
        }

        Memory memory = currentProgram.getMemory();
        MemoryBlock block = memory.getBlock(start);
        if (block == null) {
            throw new IllegalStateException("bank interval has no mapped memory block");
        }
        if (!block.getStart().getAddressSpace().equals(defaultAddressSpace) || block.isOverlay()) {
            throw new IllegalStateException("bank interval resolves through a non-default address space");
        }
        if (!block.contains(end)) {
            throw new IllegalStateException("bank interval crosses memory blocks");
        }
        if (!block.isRead()) {
            throw new IllegalStateException("bank interval is not readable");
        }

        MessageDigest digest = MessageDigest.getInstance("SHA-256");
        byte[] buffer = new byte[64 * 1024];
        long consumed = 0;
        while (consumed < length) {
            int chunkLength = (int) Math.min(buffer.length, length - consumed);
            Address chunkStart = start.addNoWrap(consumed);
            int bytesRead = memory.getBytes(chunkStart, buffer, 0, chunkLength);
            if (bytesRead != chunkLength) {
                throw new IllegalStateException(
                    "bank interval became unreadable at " + chunkStart
                );
            }
            digest.update(buffer, 0, chunkLength);
            consumed = Math.addExact(consumed, chunkLength);
        }
        String actualSha = HexFormat.of().formatHex(digest.digest());
        if (!actualSha.equals(expectedSha)) {
            throw new IllegalStateException(
                "mapped bank digest mismatch: expected " + expectedSha + ", got " + actualSha
            );
        }
    }

    private List<Candidate> collectCandidates(String bank, long vaStart, long vaEnd) {
        List<Candidate> result = new ArrayList<>();
        FunctionIterator functions = currentProgram.getFunctionManager().getFunctions(true);
        for (Function function : functions) {
            if (function.isExternal()) {
                continue;
            }
            long entry = function.getEntryPoint().getUnsignedOffset();
            AddressSetView body = function.getBody();
            if (entry < vaStart || entry >= vaEnd) {
                throw new IllegalStateException("Ghidra produced an out-of-bank function entry");
            }
            String suffix = String.format("%08x", entry);
            result.add(new Candidate(
                1, entry, entry, entry, "ghidra:function-entry:" + bank + ":" + suffix
            ));
            int rangeCount = (int) body.getNumAddressRanges();
            int rangeIndex = 0;
            for (AddressRange range : body.getAddressRanges(true)) {
                Address rangeMin = range.getMinAddress();
                Address rangeMax = range.getMaxAddress();
                if (!rangeMin.getAddressSpace().equals(function.getEntryPoint().getAddressSpace()) ||
                        !rangeMax.getAddressSpace().equals(function.getEntryPoint().getAddressSpace())) {
                    throw new IllegalStateException(
                        "function body crosses address spaces at " + function.getEntryPoint()
                    );
                }
                long bodyStart = rangeMin.getUnsignedOffset();
                long bodyMax = rangeMax.getUnsignedOffset();
                if (bodyMax >= 0xffff_ffffL) {
                    throw new IllegalStateException(
                        "function body range end overflows u32 at " + function.getEntryPoint()
                    );
                }
                long bodyEnd = bodyMax + 1;
                if (bodyStart < vaStart || bodyEnd <= bodyStart || bodyEnd > vaEnd ||
                        (bodyStart & 3) != 0 || (bodyEnd & 3) != 0) {
                    long clippedStart = Math.max(bodyStart, vaStart);
                    long clippedEnd = Math.min(bodyEnd, vaEnd);
                    if (clippedStart < clippedEnd && (clippedStart & 3) == 0 &&
                            (clippedEnd & 3) == 0) {
                        skippedRanges.add(new SkippedRange(clippedStart, clippedEnd));
                    }
                    skippedBodyEntries.add(entry);
                    rangeIndex++;
                    continue;
                }
                if (rangeCount == 1) {
                    if (bodyStart != entry) {
                        throw new IllegalStateException(
                            "contiguous function body does not begin at its entry at " +
                            function.getEntryPoint()
                        );
                    }
                    result.add(new Candidate(
                        2, entry, bodyStart, bodyEnd,
                        "ghidra:function-extent:" + bank + ":" + suffix
                    ));
                } else {
                    result.add(new Candidate(
                        6, entry, bodyStart, bodyEnd,
                        "ghidra:function-body-range:" + bank + ":" + suffix + ":" +
                        String.format("%04x", rangeIndex)
                    ));
                }
                rangeIndex++;
            }
            if (rangeIndex != rangeCount || rangeCount == 0) {
                throw new IllegalStateException(
                    "function body range count changed at " + function.getEntryPoint()
                );
            }
        }
        result.sort(Comparator.comparingLong(Candidate::start)
            .thenComparingInt(Candidate::tag)
            .thenComparing(Candidate::providerId));
        return result;
    }

    private List<Candidate> collectExecutableRanges(String bank, long vaStart, long vaEnd) {
        List<Candidate> result = new ArrayList<>();
        for (MemoryBlock block : currentProgram.getMemory().getBlocks()) {
            if (!block.isExecute()) continue;
            long start = Math.max(vaStart, block.getStart().getOffset());
            long end = Math.min(vaEnd, block.getEnd().getOffset() + 1);
            start = (start + 3) & ~3L;
            end &= ~3L;
            if (start < end) {
                result.add(new Candidate(3, 0, start, end,
                    "ghidra:executable-range:" + bank + ":" +
                    String.format("%08x-%08x", start, end)));
            }
        }
        return result;
    }

    private String serializeSkippedRanges(String bank) {
        StringBuilder result = new StringBuilder("[");
        for (int index = 0; index < skippedRanges.size(); index++) {
            if (index != 0) {
                result.append(',');
            }
            SkippedRange skipped = skippedRanges.get(index);
            result.append("{\"bank\":\"").append(json(bank))
                .append("\",\"va_start\":").append(skipped.start())
                .append(",\"va_end\":").append(skipped.end()).append('}');
        }
        return result.append(']').toString();
    }

    private String serializeSkippedBodyWarnings() {
        StringBuilder result = new StringBuilder("[");
        for (int index = 0; index < skippedBodyEntries.size(); index++) {
            if (index != 0) {
                result.append(',');
            }
            result.append("\"cross_bank_function_body:")
                .append(String.format("%08x", skippedBodyEntries.get(index)))
                .append('"');
        }
        return result.append(']').toString();
    }

    private static String claimsDigest(String bank, List<Candidate> candidates) throws Exception {
        MessageDigest digest = MessageDigest.getInstance("SHA-256");
        digest.update("fn64.tool-adapter.claim-records.v1\0".getBytes(StandardCharsets.UTF_8));
        putU64(digest, candidates.size());
        for (int sequence = 0; sequence < candidates.size(); sequence++) {
            Candidate candidate = candidates.get(sequence);
            putU64(digest, sequence);
            putString(digest, candidate.providerId());
            digest.update((byte) candidate.tag());
            if (candidate.tag() == 6) {
                putString(digest, bank);
                putU32(digest, candidate.entry());
            }
            putString(digest, bank);
            putU32(digest, candidate.start());
            if (candidate.tag() == 2 || candidate.tag() == 3 || candidate.tag() == 6) {
                putU32(digest, candidate.end());
            }
        }
        return HexFormat.of().formatHex(digest.digest());
    }

    private static void putString(MessageDigest digest, String value) {
        byte[] bytes = value.getBytes(StandardCharsets.UTF_8);
        putU64(digest, bytes.length);
        digest.update(bytes);
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
        if (value.isEmpty() || value.length() > 128 || value.chars().anyMatch(Character::isISOControl)) {
            throw new IllegalArgumentException("invalid " + label);
        }
        return value;
    }

    private static String json(String value) {
        return value.replace("\\", "\\\\").replace("\"", "\\\"");
    }
}
