// Export Ghidra function candidates through fn64's strict candidate-only JSONL schema.
// @category fn64

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.AddressSetView;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionIterator;

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
    private record Candidate(int tag, long start, long end, String providerId) {}

    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length != 13 && args.length != 15) {
            throw new IllegalArgumentException(
                "usage: OUT MODE BANK VA_START VA_END ROM_SHA BANK_SHA MAPPING_SHA " +
                "GHIDRA_VERSION BUILD_SHA CONFIG_SHA EVIDENCE_SHA [SNAPSHOT_ROLE SNAPSHOT_SHA]"
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
        if (mode.equals("seeded") != (args.length == 15)) {
            throw new IllegalArgumentException("only seeded runs carry discovery snapshot lineage");
        }

        String snapshotRole = null;
        String snapshotSha = null;
        if (args.length == 15) {
            snapshotRole = args[13];
            if (!snapshotRole.equals("discovery_snapshot")) {
                throw new IllegalArgumentException("seeded lineage role must be discovery_snapshot");
            }
            snapshotSha = requireSha(args[14]);
        }

        List<Candidate> candidates = collectCandidates(bank, vaStart, vaEnd);
        String claimsSha = claimsDigest(bank, candidates);
        String toolName = "ghidra-headless-" + mode;

        try (BufferedWriter writer = new BufferedWriter(new OutputStreamWriter(
                new FileOutputStream(output), StandardCharsets.UTF_8))) {
            writer.write("{\"record\":\"header\",\"schema\":\"fn64.tool-adapter\",\"schema_version\":1");
            writer.write(",\"tool\":{\"name\":\"" + toolName + "\",\"version\":\"" +
                json(ghidraVersion) + "\",\"build_sha256\":\"" + buildSha + "\"}");
            writer.write(",\"role\":\"function_boundary_candidates\"");
            writer.write(",\"input\":{\"normalized_rom_sha256\":\"" + romSha +
                "\",\"bank\":\"" + json(bank) + "\",\"bank_bytes_sha256\":\"" + bankSha +
                "\",\"mapping_sha256\":\"" + mappingSha + "\",\"va_start\":" + vaStart +
                ",\"va_end\":" + vaEnd + "}");
            writer.write(",\"lineage\":[{\"role\":\"tool_configuration\",\"source_sha256\":\"" +
                configSha + "\"},{\"role\":\"evidence_manifest\",\"source_sha256\":\"" +
                evidenceSha + "\"}");
            if (snapshotSha != null) {
                writer.write(", {\"role\":\"" + snapshotRole + "\",\"source_sha256\":\"" +
                    snapshotSha + "\"}");
            }
            writer.write("]}\n");

            for (int sequence = 0; sequence < candidates.size(); sequence++) {
                Candidate candidate = candidates.get(sequence);
                writer.write("{\"record\":\"claim\",\"sequence\":" + sequence +
                    ",\"provider_claim_id\":\"" + json(candidate.providerId()) + "\",\"claim\":");
                if (candidate.tag() == 1) {
                    writer.write("{\"type\":\"function_entry\",\"address\":{\"bank\":\"" +
                        json(bank) + "\",\"pc\":" + candidate.start() + "}}");
                } else {
                    writer.write("{\"type\":\"function_extent\",\"range\":{\"bank\":\"" +
                        json(bank) + "\",\"va_start\":" + candidate.start() +
                        ",\"va_end\":" + candidate.end() + "}}");
                }
                writer.write("}\n");
            }

            writer.write("{\"record\":\"summary\",\"complete\":true,\"analyzed_range\":{\"bank\":\"" +
                json(bank) + "\",\"va_start\":" + vaStart + ",\"va_end\":" + vaEnd +
                "},\"skipped_ranges\":[],\"claim_records\":" + candidates.size() +
                ",\"claims_sha256\":\"" + claimsSha + "\",\"resources\":{\"input_bytes\":" +
                (vaEnd - vaStart) + ",\"elapsed_millis\":0,\"peak_memory_bytes\":null," +
                "\"limit_hit\":false,\"warnings\":[]}}\n");
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
            if (body.getNumAddressRanges() != 1) {
                throw new IllegalStateException(
                    "schema v1 cannot represent a discontiguous function body at " +
                    function.getEntryPoint()
                );
            }
            long bodyStart = body.getMinAddress().getUnsignedOffset();
            long bodyEnd = body.getMaxAddress().getUnsignedOffset() + 1;
            if (bodyStart != entry || bodyEnd <= bodyStart || bodyEnd > vaEnd ||
                    (bodyStart & 3) != 0 || (bodyEnd & 3) != 0) {
                throw new IllegalStateException(
                    "function body violates the supplied bank mapping at " + function.getEntryPoint()
                );
            }
            String suffix = String.format("%08x", entry);
            result.add(new Candidate(1, entry, entry, "ghidra:function-entry:" + bank + ":" + suffix));
            result.add(new Candidate(2, bodyStart, bodyEnd, "ghidra:function-extent:" + bank + ":" + suffix));
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
