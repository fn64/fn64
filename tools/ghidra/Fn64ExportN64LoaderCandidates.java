// Export N64LoaderWV first-contact results as bank-local, candidate-only claims.
// @category fn64

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
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

public class Fn64ExportN64LoaderCandidates extends GhidraScript {
    private record Candidate(int tag, long start, long end, String providerId) {}

    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length != 12) {
            throw new IllegalArgumentException(
                "usage: OUT BANK VA_START VA_END ROM_SHA BANK_SHA MAPPING_SHA " +
                "LOADER_COMMIT EXTENSION_SHA CONFIG_SHA EVIDENCE_SHA PROGRAM_NAME"
            );
        }

        String output = args[0];
        String bank = requireToken(args[1], "bank");
        long vaStart = parseU32(args[2]);
        long vaEnd = parseU32(args[3]);
        if (vaStart >= vaEnd || (vaStart & 3) != 0 || (vaEnd & 3) != 0) {
            throw new IllegalArgumentException("invalid bank interval");
        }
        String romSha = requireSha(args[4]);
        String bankSha = requireSha(args[5]);
        String mappingSha = requireSha(args[6]);
        String loaderCommit = requireCommit(args[7]);
        String extensionSha = requireSha(args[8]);
        String configSha = requireSha(args[9]);
        String evidenceSha = requireSha(args[10]);
        String expectedProgramName = requireToken(args[11], "program name");
        if (!currentProgram.getName().equals(expectedProgramName)) {
            throw new IllegalStateException("wrong program: " + currentProgram.getName());
        }

        AddressSpace defaultAddressSpace =
            currentProgram.getAddressFactory().getDefaultAddressSpace();
        verifyMappedBank(defaultAddressSpace, vaStart, vaEnd, bankSha);
        List<Candidate> candidates = collectCandidates(bank, vaStart, vaEnd);
        String claimsSha = claimsDigest(bank, candidates);

        try (BufferedWriter writer = new BufferedWriter(new OutputStreamWriter(
                new FileOutputStream(output), StandardCharsets.UTF_8))) {
            writer.write("{\"record\":\"header\",\"schema\":\"fn64.tool-adapter\",\"schema_version\":1");
            writer.write(",\"tool\":{\"name\":\"n64loaderwv-first-contact\",\"version\":\"" +
                loaderCommit + "\",\"build_sha256\":\"" + extensionSha + "\"}");
            writer.write(",\"role\":\"function_boundary_candidates\"");
            writer.write(",\"input\":{\"normalized_rom_sha256\":\"" + romSha +
                "\",\"bank\":\"" + json(bank) + "\",\"bank_bytes_sha256\":\"" + bankSha +
                "\",\"mapping_sha256\":\"" + mappingSha + "\",\"va_start\":" + vaStart +
                ",\"va_end\":" + vaEnd + "}");
            writer.write(",\"lineage\":[{\"role\":\"tool_configuration\",\"source_sha256\":\"" +
                configSha + "\"},{\"role\":\"evidence_manifest\",\"source_sha256\":\"" +
                evidenceSha + "\"}]}");
            writer.newLine();

            for (int sequence = 0; sequence < candidates.size(); sequence++) {
                Candidate candidate = candidates.get(sequence);
                writer.write("{\"record\":\"claim\",\"sequence\":" + sequence +
                    ",\"provider_claim_id\":\"" + json(candidate.providerId()) + "\",\"claim\":");
                if (candidate.tag() == 1) {
                    writer.write("{\"type\":\"function_entry\",\"address\":{\"bank\":\"" +
                        json(bank) + "\",\"pc\":" + candidate.start() + "}}");
                }
                else {
                    writer.write("{\"type\":\"function_extent\",\"range\":{\"bank\":\"" +
                        json(bank) + "\",\"va_start\":" + candidate.start() +
                        ",\"va_end\":" + candidate.end() + "}}");
                }
                writer.write("}");
                writer.newLine();
            }

            writer.write("{\"record\":\"summary\",\"complete\":true,\"analyzed_range\":{\"bank\":\"" +
                json(bank) + "\",\"va_start\":" + vaStart + ",\"va_end\":" + vaEnd +
                "},\"skipped_ranges\":[],\"claim_records\":" + candidates.size() +
                ",\"claims_sha256\":\"" + claimsSha + "\",\"resources\":{\"input_bytes\":" +
                (vaEnd - vaStart) + ",\"elapsed_millis\":0,\"peak_memory_bytes\":null," +
                "\"limit_hit\":false,\"warnings\":[]}}");
            writer.newLine();
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
            if (entry < vaStart || entry >= vaEnd) {
                continue;
            }

            AddressSetView body = function.getBody();
            if (body.getNumAddressRanges() != 1) {
                throw new IllegalStateException(
                    "selected-bank function has a discontiguous body at " + function.getEntryPoint()
                );
            }
            long bodyStart = body.getMinAddress().getUnsignedOffset();
            long bodyEnd = body.getMaxAddress().getUnsignedOffset() + 1;
            if (bodyStart != entry || bodyEnd <= bodyStart || bodyStart < vaStart || bodyEnd > vaEnd ||
                    (bodyStart & 3) != 0 || (bodyEnd & 3) != 0) {
                throw new IllegalStateException(
                    "selected-bank function body violates the supplied bank mapping at " +
                    function.getEntryPoint()
                );
            }

            String suffix = String.format("%08x", entry);
            result.add(new Candidate(
                1, entry, entry, "n64loaderwv:function-entry:" + bank + ":" + suffix
            ));
            result.add(new Candidate(
                2, bodyStart, bodyEnd, "n64loaderwv:function-extent:" + bank + ":" + suffix
            ));
        }
        result.sort(Comparator.comparingLong(Candidate::start)
            .thenComparingInt(Candidate::tag)
            .thenComparing(Candidate::providerId));
        return result;
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
            putString(digest, bank);
            putU32(digest, candidate.start());
            if (candidate.tag() == 2) {
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
        return value.replace("\\", "\\\\").replace("\"", "\\\"");
    }
}
