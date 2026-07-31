# Validate one bounded WM2000 pure-AOT scenario log and emit only canonical,
# deterministic evidence suitable for byte comparison across repeated runs.

BEGIN {
    failed = 0
    input_edges = 0
    if (min_input_edges == "") min_input_edges = 1
    if (min_controller_ops == "") min_controller_ops = 1
    if (min_standard_reads == "") min_standard_reads = 1
    if (min_gfx == "") min_gfx = 1
    if (min_audio == "") min_audio = 0
    if (min_rcp_completed == "") min_rcp_completed = 1
    if (min_ucode == "") min_ucode = 1
    if (min_dpc_commits == "") min_dpc_commits = 1
}

function reject(message) {
    print "wm2000 scenario gate: " message > "/dev/stderr"
    failed = 1
}

function numeric_field(line, name,    fields, count, field_index, pair) {
    count = split(line, fields, /[[:space:]]+/)
    for (field_index = 1; field_index <= count; field_index++) {
        split(fields[field_index], pair, "=")
        if (pair[1] == name && pair[2] ~ /^[0-9]+$/) return pair[2] + 0
    }
    return -1
}

function string_field(line, name,    fields, count, field_index, pair) {
    count = split(line, fields, /[[:space:]]+/)
    for (field_index = 1; field_index <= count; field_index++) {
        split(fields[field_index], pair, "=")
        if (pair[1] == name) return pair[2]
    }
    return ""
}

{
    lowered = tolower($0)
    if (lowered ~ /aotmiss|missingaotentry|imagechanged|unknownbank|unknown executable bank|unsupportedinstruction|unsupportedop|unsupported cpu|unsupported rsp|unsupported device|static-dispatch failure/) {
        reject("static execution failure marker: " $0)
    }
}

/\[wm2000-block-boot\] static execution build / {
    build_lines++
    build_schema = numeric_field($0, "schema")
    build_aot_runtime = string_field($0, "aot_runtime")
    build_production_aot = string_field($0, "production_aot")
    build_dev_interpreter = string_field($0, "dev_interpreter")
}

/\[wm2000-block-boot\] canonical program artifact=/ {
    artifact_lines++
    artifact = string_field($0, "artifact")
}

/\[wm2000-block-boot\] controller schedule=/ {
    schedule_lines++
    schedule_phases = numeric_field($0, "phases")
    schedule_sha256 = string_field($0, "sha256")
}

/\[wm2000-block-boot\] controller input_edge / {
    input_edges++
    input_edge[input_edges] = $0
}

/\[wm2000-block-boot\] done:/ {
    done_lines++
    done = $0
}

/\[wm2000-block-boot\] standard controller reads / {
    standard_read_lines++
    standard_reads = numeric_field($0, "port0") + numeric_field($0, "port1") + numeric_field($0, "port2") + numeric_field($0, "port3")
    standard_read_summary = $0
}

/\[wm2000-block-progress\]/ {
    progress_lines++
    progress = $0
    gfx = numeric_field($0, "gfx_submits")
    audio = numeric_field($0, "audio_submits")
    controller_ops = numeric_field($0, "controller_ops")
    rcp_completed = numeric_field($0, "rcp_completed")
    ucode = numeric_field($0, "ucode_recognitions")
    dram_dpc = numeric_field($0, "dram_dpc")
    xbus_dpc = numeric_field($0, "xbus_dpc")
    if ($0 !~ /render_error=None/) reject("render_error is not None")
}

/\[wm2000-block-profile\] phase_timing / {
    timing_lines++
    executor_calls = numeric_field($0, "calls")
}

/\[wm2000-block-boot\] entered digest-selected ROM-recovered generations:/ {
    generation_lines++
    generations = $0
    generation_values = $0
    sub(/^.*\[/, "", generation_values)
    sub(/\][^]]*$/, "", generation_values)
    entered_count = split(generation_values, entered, ",")
    for (entered_index = 1; entered_index <= entered_count; entered_index++) {
        gsub(/[[:space:]]/, "", entered[entered_index])
        if (entered[entered_index] != "") entered_generation[entered[entered_index]] = 1
    }
}

/\[wm2000-block-boot\] bounded progress-only exit:/ {
    exit_lines++
    bounded_exit = $0
}

END {
    if (build_lines != 1) reject("expected exactly one static-execution build receipt")
    if (build_schema != 1 || build_aot_runtime != "true" || build_production_aot != "true" || build_dev_interpreter != "false") {
        reject("linked CPU lane is not production AOT-only")
    }
    if (artifact_lines != 1) reject("expected exactly one canonical program artifact line")
    if (length(artifact) != 64 || artifact !~ /^[0-9a-f]+$/) reject("program artifact identity is not 32-byte lowercase hex")
    if (schedule_lines != 1) reject("expected exactly one controller schedule identity line")
    if (schedule_phases < 1 || length(schedule_sha256) != 64 || schedule_sha256 !~ /^[0-9a-f]+$/) {
        reject("controller schedule identity is malformed")
    }
    if (done_lines != 1) reject("expected exactly one bounded completion line")
    if (standard_read_lines != 1) reject("expected exactly one standard-controller read summary")
    if (progress_lines != 1) reject("expected exactly one runtime progress summary")
    if (generation_lines != 1) reject("expected exactly one entered-generation summary")
    if (exit_lines != 1) reject("expected exactly one bounded process-exit line")
    if (timing_lines != 1) reject("expected exactly one phase-timing checkpoint")
    if (input_edges < min_input_edges) reject("controller input edges below required minimum")
    if (controller_ops < min_controller_ops) reject("controller operations below required minimum")
    if (standard_reads < min_standard_reads) reject("completed standard-controller reads below required minimum")
    if (gfx < min_gfx) reject("graphics submissions below required minimum")
    if (audio < min_audio) reject("audio submissions below required minimum")
    if (rcp_completed < min_rcp_completed) reject("completed RCP tasks below required minimum")
    if (ucode < min_ucode) reject("microcode recognitions below required minimum")
    if (dram_dpc + xbus_dpc < min_dpc_commits) reject("committed graphics DPC streams below required minimum")
    if (executor_calls < 1) reject("executor timing checkpoint has no calls")

    required_count = split(required_generations, required, ",")
    if (required_generations != "") {
        for (required_index = 1; required_index <= required_count; required_index++) {
            if (!(required[required_index] in entered_generation)) {
                reject("required overlay generation absent: " required[required_index])
            }
        }
    }

    if (failed) exit 1

    print "static execution build schema=1 aot_runtime=true production_aot=true dev_interpreter=false"
    print "program artifact=" artifact
    print "schedule phases=" schedule_phases " sha256=" schedule_sha256
    for (edge_index = 1; edge_index <= input_edges; edge_index++) print input_edge[edge_index]
    print done
    print standard_read_summary
    print progress
    print generations
    print bounded_exit
}
