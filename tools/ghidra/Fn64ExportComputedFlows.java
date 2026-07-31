// Export non-exhaustive computed-control-flow candidates through fn64 schema v3.
// @category fn64

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.address.AddressSpace;
import ghidra.program.model.lang.Register;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.listing.InstructionIterator;
import ghidra.program.model.mem.Memory;
import ghidra.program.model.mem.MemoryBlock;
import ghidra.program.model.symbol.FlowType;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.RefType;

import java.io.BufferedWriter;
import java.io.FileOutputStream;
import java.io.OutputStreamWriter;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.ArrayList;
import java.util.HexFormat;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;
import java.util.TreeSet;

public class Fn64ExportComputedFlows extends GhidraScript {
    private record ComputedFlow(long site, boolean viaCall, List<Long> targets) {}

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
        String mode = requireToken(args[1], "mode");
        if (!mode.equals("unseeded") && !mode.equals("seeded") &&
                !mode.equals("discovery_only")) {
            throw new IllegalArgumentException(
                "mode must be unseeded, seeded, or discovery_only"
            );
        }
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
        String snapshotRole = args[13];
        if (!snapshotRole.equals("discovery_snapshot")) {
            throw new IllegalArgumentException("lineage role must be discovery_snapshot");
        }
        String snapshotSha = requireSha(args[14]);

        AddressSpace defaultAddressSpace =
            currentProgram.getAddressFactory().getDefaultAddressSpace();
        verifyMappedBank(defaultAddressSpace, vaStart, vaEnd, bankSha);
        int[] outOfBankReferences = {0};
        List<ComputedFlow> flows = collectComputedFlows(
            defaultAddressSpace, vaStart, vaEnd, outOfBankReferences
        );
        String claimsSha = claimsDigest(bank, flows);
        String toolName = "ghidra-headless-" + mode + "-computed-flow";

        try (BufferedWriter writer = new BufferedWriter(new OutputStreamWriter(
                new FileOutputStream(output), StandardCharsets.UTF_8))) {
            writer.write("{\"record\":\"header\",\"schema\":\"fn64.tool-adapter\",\"schema_version\":3");
            writer.write(",\"tool\":{\"name\":\"" + json(toolName) + "\",\"version\":\"" +
                json(ghidraVersion) + "\",\"build_sha256\":\"" + buildSha + "\"}");
            writer.write(",\"role\":\"control_flow_candidates\"");
            writer.write(",\"input\":{\"normalized_rom_sha256\":\"" + romSha +
                "\",\"bank\":\"" + json(bank) + "\",\"bank_bytes_sha256\":\"" + bankSha +
                "\",\"mapping_sha256\":\"" + mappingSha + "\",\"va_start\":" + vaStart +
                ",\"va_end\":" + vaEnd + "}");
            writer.write(",\"lineage\":[{\"role\":\"tool_configuration\",\"source_sha256\":\"" +
                configSha + "\"},{\"role\":\"evidence_manifest\",\"source_sha256\":\"" +
                evidenceSha + "\"},{\"role\":\"" + snapshotRole +
                "\",\"source_sha256\":\"" + snapshotSha + "\"}]}");
            writer.newLine();

            for (int sequence = 0; sequence < flows.size(); sequence++) {
                ComputedFlow flow = flows.get(sequence);
                String suffix = String.format("%08x", flow.site());
                writer.write("{\"record\":\"claim\",\"sequence\":" + sequence +
                    ",\"provider_claim_id\":\"ghidra:computed-flow:" + json(bank) + ":" +
                    suffix + "\",\"claim\":{\"type\":\"computed_control_flow\"," +
                    "\"site\":{\"bank\":\"" + json(bank) + "\",\"pc\":" + flow.site() +
                    "},\"via_call\":" + flow.viaCall() + ",\"targets\":[");
                for (int targetIndex = 0; targetIndex < flow.targets().size(); targetIndex++) {
                    if (targetIndex != 0) {
                        writer.write(",");
                    }
                    writer.write("{\"bank\":\"" + json(bank) + "\",\"pc\":" +
                        flow.targets().get(targetIndex) + "}");
                }
                writer.write("],\"completeness\":\"unknown\"}}");
                writer.newLine();
            }

            writer.write("{\"record\":\"summary\",\"complete\":true,\"analyzed_range\":{\"bank\":\"" +
                json(bank) + "\",\"va_start\":" + vaStart + ",\"va_end\":" + vaEnd +
                "},\"skipped_ranges\":[],\"claim_records\":" + flows.size() +
                ",\"claims_sha256\":\"" + claimsSha + "\",\"resources\":{\"input_bytes\":" +
                (vaEnd - vaStart) + ",\"elapsed_millis\":0,\"peak_memory_bytes\":null," +
                "\"limit_hit\":false,\"warnings\":[");
            if (outOfBankReferences[0] != 0) {
                writer.write("\"ignored_out_of_bank_flow_references=" + outOfBankReferences[0] + "\"");
            }
            writer.write("]}}");
            writer.newLine();
        }
    }

    private List<ComputedFlow> collectComputedFlows(
            AddressSpace defaultAddressSpace, long vaStart, long vaEnd,
            int[] outOfBankReferences) {
        Map<Long, ComputedFlow> bySite = new TreeMap<>();
        boolean rawScan = Boolean.parseBoolean(
            System.getProperty("fn64.rawIndirectCandidates", "false")
        );
        InstructionIterator instructions = currentProgram.getListing().getInstructions(true);
        for (Instruction instruction : instructions) {
            Address address = instruction.getAddress();
            if (!address.getAddressSpace().equals(defaultAddressSpace)) {
                continue;
            }
            long site = address.getUnsignedOffset();
            if (site < vaStart || site >= vaEnd) {
                continue;
            }
            FlowType flowType = instruction.getFlowType();
            // Ghidra can leave register-indirect `jr`/`jalr` instructions with
            // a non-computed flow type when no reference target was recovered.
            // Keep those sites in the candidate stream so the native closure
            // can distinguish an open site from an absent site.
            boolean rawComputed = isRegisterIndirectTransfer(instruction);
            if ((!flowType.isComputed() || (!flowType.isCall() && !flowType.isJump()))
                    && (!rawScan || !rawComputed)) {
                continue;
            }
            if (instruction.getLength() != 4 || (site & 3) != 0) {
                throw new IllegalStateException(
                    "computed-flow instruction is not one aligned MIPS word at " + address
                );
            }
            if (isOrdinaryReturn(instruction, flowType)) {
                continue;
            }

            TreeSet<Long> targets = new TreeSet<>();
            for (Reference reference : instruction.getReferencesFrom()) {
                RefType referenceType = reference.getReferenceType();
                if (!referenceType.isFlow() || !referenceType.isComputed() ||
                        referenceType.isFallthrough()) {
                    continue;
                }
                Address target = reference.getToAddress();
                if (!target.getAddressSpace().equals(defaultAddressSpace)) {
                    outOfBankReferences[0]++;
                    continue;
                }
                long targetOffset = target.getUnsignedOffset();
                if (targetOffset < vaStart || targetOffset >= vaEnd) {
                    outOfBankReferences[0]++;
                    continue;
                }
                if ((targetOffset & 3) != 0) {
                    throw new IllegalStateException(
                        "computed-flow target is not word-aligned at " + target
                    );
                }
                targets.add(targetOffset);
            }
            ComputedFlow candidate = new ComputedFlow(
                site, flowType.isCall(), List.copyOf(targets)
            );
            ComputedFlow prior = bySite.putIfAbsent(site, candidate);
            if (prior != null && !prior.equals(candidate)) {
                throw new IllegalStateException(
                    "incompatible duplicate computed-flow site at " + address
                );
            }
        }
        // Analysis may leave an indirect transfer in an undefined/data region
        // with no Instruction object. Recover the site from the raw MIPS word
        // so an unresolved transfer remains an explicit candidate.
        Memory memory = currentProgram.getMemory();
        for (long site = vaStart; rawScan && site < vaEnd; site += 4) {
            if (bySite.containsKey(site)) {
                continue;
            }
            Address address = defaultAddressSpace.getAddress(site);
            int word;
            try {
                word = memory.getInt(address);
            } catch (Exception ignored) {
                continue;
            }
            int opcode = (word >>> 26) & 0x3f;
            int function = word & 0x3f;
            int rs = (word >>> 21) & 0x1f;
            if (opcode != 0 || (function != 8 && function != 9) || rs == 31) {
                continue;
            }
            bySite.put(site, new ComputedFlow(site, function == 9, List.of()));
        }
        return new ArrayList<>(bySite.values());
    }

    private static boolean isOrdinaryReturn(Instruction instruction, FlowType flowType) {
        if (!flowType.isJump() || !instruction.getMnemonicString().equalsIgnoreCase("jr")) {
            return false;
        }
        Register register = instruction.getRegister(0);
        return register != null && register.getName().equalsIgnoreCase("ra");
    }

    private static boolean isRegisterIndirectTransfer(Instruction instruction) {
        String mnemonic = instruction.getMnemonicString();
        if (!mnemonic.equalsIgnoreCase("jr") && !mnemonic.equalsIgnoreCase("jalr")) {
            return false;
        }
        Register register = instruction.getRegister(0);
        return register != null && !register.getName().equalsIgnoreCase("ra");
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
        if (block == null || !block.getStart().getAddressSpace().equals(defaultAddressSpace) ||
                block.isOverlay() || !block.contains(end) || !block.isRead()) {
            throw new IllegalStateException("bank interval is not one readable default-space block");
        }
        MessageDigest digest = MessageDigest.getInstance("SHA-256");
        byte[] buffer = new byte[64 * 1024];
        long consumed = 0;
        while (consumed < length) {
            int chunkLength = (int) Math.min(buffer.length, length - consumed);
            int bytesRead = memory.getBytes(start.addNoWrap(consumed), buffer, 0, chunkLength);
            if (bytesRead != chunkLength) {
                throw new IllegalStateException("bank interval became unreadable");
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

    private static String claimsDigest(String bank, List<ComputedFlow> flows) throws Exception {
        MessageDigest digest = MessageDigest.getInstance("SHA-256");
        digest.update("fn64.tool-adapter.claim-records.v1\0".getBytes(StandardCharsets.UTF_8));
        putU64(digest, flows.size());
        for (int sequence = 0; sequence < flows.size(); sequence++) {
            ComputedFlow flow = flows.get(sequence);
            putU64(digest, sequence);
            putString(digest, "ghidra:computed-flow:" + bank + ":" +
                String.format("%08x", flow.site()));
            digest.update((byte) 7);
            putAddress(digest, bank, flow.site());
            digest.update((byte) (flow.viaCall() ? 1 : 0));
            putU64(digest, flow.targets().size());
            for (long target : flow.targets()) {
                putAddress(digest, bank, target);
            }
            digest.update((byte) 0);
        }
        return HexFormat.of().formatHex(digest.digest());
    }

    private static void putAddress(MessageDigest digest, String bank, long pc) {
        putString(digest, bank);
        putU32(digest, pc);
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
