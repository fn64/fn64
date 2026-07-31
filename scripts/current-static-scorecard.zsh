#!/bin/zsh

# Produce one caller-attested current WM2000 static-recompilation scorecard.
# Private producer output is retained in the caller-selected directory. The
# path-free aggregate and opt-in full-audit progress are written to stdout.

set -eu

typeset -r script_path=$0
typeset -r repo_root=${0:A:h:h}
typeset output_dir=
typeset mode=run
typeset -i full_writer_audit=0
typeset -i image_group_count=0
typeset -i build_timeout_seen=0
typeset build_timeout=
typeset -a image_group_arguments
typeset -a image_group_names_ordered
typeset -A image_group_names
typeset -A image_group_path_lists

usage() {
    print -u2 -- "usage: $script_path --output ABSOLUTE_EMPTY_DIRECTORY"
    print -u2 -- "       $script_path --full-writer-audit --image-group NAME ABS ABS ABS [ABS ...] [--image-group ...] [--max-build-seconds 2400..7200] --output ABSOLUTE_EMPTY_DIRECTORY"
    print -u2 -- "       $script_path --dry-run --output ABSOLUTE_EMPTY_DIRECTORY"
    print -u2 -- "       $script_path --selftest"
}

while (( $# > 0 )); do
    case $1 in
        --output)
            (( $# >= 2 )) || { usage; exit 2; }
            output_dir=$2
            shift 2
            ;;
        --dry-run)
            mode=dry-run
            shift
            ;;
        --full-writer-audit)
            full_writer_audit=1
            shift
            ;;
        --image-group)
            (( $# >= 2 )) || { usage; exit 2; }
            typeset group_name=$2
            [[ $group_name =~ '^FN64_EXECUTABLE_IMAGE_[A-Z0-9_]+$' \
                && -z ${image_group_names[$group_name]:-} ]] || {
                print -u2 -- "current static scorecard: image-group names must be unique FN64_EXECUTABLE_IMAGE_* tokens"
                exit 2
            }
            image_group_names[$group_name]=1
            image_group_arguments+=(--image-group $group_name)
            shift 2
            typeset -i capture_count=0
            typeset -a group_captures
            group_captures=()
            while (( $# > 0 )) && [[ $1 != --* ]]; do
                [[ $1 == /* && $1 != *'/../'* && $1 != */.. \
                    && -f $1 && -r $1 ]] || {
                    print -u2 -- "current static scorecard: image captures must be absolute readable files without parent traversal"
                    exit 2
                }
                [[ $1 != *:* ]] || {
                    print -u2 -- "current static scorecard: image capture paths must not contain ':' because the static-frontier path-list wire cannot represent it"
                    exit 2
                }
                image_group_arguments+=($1)
                group_captures+=($1)
                (( capture_count += 1 ))
                shift
            done
            (( capture_count >= 3 )) || {
                print -u2 -- "current static scorecard: each --image-group requires at least three captures"
                exit 2
            }
            image_group_names_ordered+=($group_name)
            image_group_path_lists[$group_name]=${(j/:/)group_captures}
            (( image_group_count += 1 ))
            ;;
        --max-build-seconds)
            (( $# >= 2 && ! build_timeout_seen )) || { usage; exit 2; }
            [[ $2 == <-> && $2 -ge 2400 && $2 -le 7200 ]] || {
                print -u2 -- "current static scorecard: --max-build-seconds must be 2400..7200"
                exit 2
            }
            build_timeout=$2
            build_timeout_seen=1
            shift 2
            ;;
        --selftest)
            mode=selftest
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            print -u2 -- "current static scorecard: unknown argument"
            usage
            exit 2
            ;;
    esac
done

if [[ $mode == selftest ]]; then
    [[ -z $output_dir ]] && (( ! full_writer_audit && image_group_count == 0 \
        && ! build_timeout_seen )) || {
        print -u2 -- "current static scorecard: --selftest does not accept run options"
        exit 2
    }
    exec "$repo_root/scripts/test-current-static-scorecard.zsh"
fi

if (( full_writer_audit )); then
    (( image_group_count > 0 )) || {
        print -u2 -- "current static scorecard: --full-writer-audit requires at least one --image-group"
        exit 2
    }
else
    (( image_group_count == 0 && ! build_timeout_seen )) || {
        print -u2 -- "current static scorecard: image groups and build timeout require --full-writer-audit"
        exit 2
    }
fi

[[ -n $output_dir && $output_dir == /* ]] || {
    print -u2 -- "current static scorecard: --output must name an explicit absolute directory"
    exit 2
}
[[ -n ${FN64_DISCOVER_NWXE_ROM:-} && $FN64_DISCOVER_NWXE_ROM == /* \
    && -f $FN64_DISCOVER_NWXE_ROM && -r $FN64_DISCOVER_NWXE_ROM ]] || {
    print -u2 -- "current static scorecard: FN64_DISCOVER_NWXE_ROM must name an absolute readable file"
    exit 2
}
[[ -n ${FN64_BOOT_CONTEXT:-} && $FN64_BOOT_CONTEXT == /* \
    && -f $FN64_BOOT_CONTEXT && -r $FN64_BOOT_CONTEXT ]] || {
    print -u2 -- "current static scorecard: FN64_BOOT_CONTEXT must name an absolute readable file"
    exit 2
}

typeset -r output_parent=${output_dir:h}
[[ -d $output_parent && -r $output_parent && -w $output_parent ]] || {
    print -u2 -- "current static scorecard: output parent must be an existing readable and writable directory"
    exit 2
}
typeset -r canonical_parent=${output_parent:A}
typeset -r canonical_repo=${repo_root:A}
if [[ $canonical_parent == $canonical_repo || $canonical_parent == $canonical_repo/* ]]; then
    print -u2 -- "current static scorecard: output directory must be outside the repository"
    exit 2
fi
[[ ! -e $output_dir && ! -L $output_dir ]] || {
    print -u2 -- "current static scorecard: output directory must not already exist"
    exit 2
}

if [[ $mode == dry-run ]]; then
    print -- "fn64.current-static-scorecard.v1 mode=dry-run output=<PRIVATE> rom=<ROM> boot_context=<BOOT_CONTEXT>"
    print -- "stage=closure_source_writer_producer guard=memory-guard cargo_jobs=1 discovery_passes=1"
    if (( full_writer_audit )); then
        print -- "stage=full_writer_audit guard=memory-guard selected_build_cargo_jobs=2 max_rss_mib=4096 image_groups=$image_group_count"
    fi
    print -- "stage=scorecard_aggregation evidence_label=current caller_attested=true"
    exit 0
fi

mkdir -m 700 -- "$output_dir" 2>/dev/null || {
    print -u2 -- "current static scorecard: cannot create private output directory"
    exit 1
}
chmod 700 "$output_dir" 2>/dev/null || {
    print -u2 -- "current static scorecard: cannot seal private output directory permissions"
    exit 1
}

private_mode() {
    local value
    if value=$(stat -f '%Lp' -- "$output_dir" 2>/dev/null); then
        [[ $value == 700 ]]
    elif value=$(stat -c '%a' -- "$output_dir" 2>/dev/null); then
        [[ $value == 700 ]]
    else
        return 1
    fi
}
private_mode || {
    print -u2 -- "current static scorecard: private output directory is not mode 0700"
    exit 1
}

typeset -r closure_receipt=$output_dir/nwxe.closure-audit-v3.json
typeset -r source_receipt=$output_dir/source.json
typeset -r writer_receipt=$output_dir/writers.json
typeset -r writer_audit_dir=$output_dir/full-writer-audit
typeset -r completed_writer_receipt=$writer_audit_dir/writers.json
typeset -r scorecard_receipt=$output_dir/scorecard.json
typeset -r guard_max_rss_mib=${FN64_GUARD_MAX_RSS_MIB:-2048}
typeset -r selected_build_guard_max_rss_mib=4096
typeset -r guard_min_free_percent=${FN64_GUARD_MIN_FREE_PERCENT:-40}
typeset -r cargo_profile_dev_debug=${CARGO_PROFILE_DEV_DEBUG:-1}

# Remove every unrelated ROM/dump input used by discovery gates, the optional
# block-program content emitter, and both executable-image capture selectors.
typeset -a clean_environment
clean_environment=(
    env
    -u FN64_DISCOVER_ROM
    -u FN64_DISCOVER_NW4E_ROM -u FN64_DISCOVER_NW4E_DUMP
    -u FN64_DISCOVER_NWXE_DUMP
    -u FN64_DISCOVER_OOT_ROM -u FN64_DISCOVER_OOT_DUMP
    -u FN64_DISCOVER_MM_ROM -u FN64_DISCOVER_MM_DUMP
    -u FN64_DISCOVER_K64_ROM -u FN64_DISCOVER_K64_DUMP
    -u FN64_DISCOVER_GE_ROM -u FN64_DISCOVER_GE_DUMP
    -u FN64_DISCOVER_PD_ROM -u FN64_DISCOVER_PD_DUMP
    -u FN64_DISCOVER_SM64_ROM -u FN64_DISCOVER_SM64_DUMP
    -u FN64_DISCOVER_WCWWT_ROM -u FN64_DISCOVER_WCWWT_DUMP
    -u FN64_DISCOVER_BANJO_ROM -u FN64_DISCOVER_BANJO_DUMP
    -u FN64_DISCOVER_SIG_DONOR_ROM -u FN64_DISCOVER_SIG_DONOR_DUMP
    -u FN64_EXECUTABLE_IMAGE_GROUPS -u FN64_EXECUTABLE_IMAGES
    -u FN64_EMIT_BLOCK_PROGRAM
)

typeset -a frontier_command writer_command scorecard_command
if [[ ${FN64_CURRENT_SCORECARD_SELFTEST_DRIVER:-} != '' ]]; then
    typeset -r test_driver=$FN64_CURRENT_SCORECARD_SELFTEST_DRIVER
    [[ ${FN64_CURRENT_SCORECARD_SELFTEST_MODE:-} == 1 \
        && $test_driver == /* && -x $test_driver/fake-static-frontier \
        && -x $test_driver/fake-writer-audit \
        && -x $test_driver/fake-scorecard ]] || {
        print -u2 -- "current static scorecard: invalid self-test driver"
        exit 2
    }
    frontier_command=($test_driver/fake-static-frontier)
    writer_command=($test_driver/fake-writer-audit)
    scorecard_command=($test_driver/fake-scorecard)
else
    [[ ${FN64_CURRENT_SCORECARD_SELFTEST_MODE:-} != 1 ]] || {
        print -u2 -- "current static scorecard: self-test mode requires its private driver"
        exit 2
    }
    frontier_command=("$repo_root/scripts/wm2000-static-frontier.zsh")
    writer_command=("$repo_root/scripts/memory-guard.zsh" cargo run -q -j1 -p fn64-discover --features writer-runtime-authority --bin run_wm_writer_audit --)
    scorecard_command=("$repo_root/scripts/static-recomp-scorecard.py")
fi

typeset -a frontier_image_environment
if (( full_writer_audit )); then
    frontier_image_environment+=(
        "FN64_EXECUTABLE_IMAGE_GROUPS=${(j:,:)image_group_names_ordered}"
    )
    for group_name in $image_group_names_ordered; do
        frontier_image_environment+=(
            "$group_name=${image_group_path_lists[$group_name]}"
        )
    done
fi

if (( full_writer_audit )); then
    print -- "current static scorecard: stage 1/3 closure, source, and writer producer"
else
    print -- "current static scorecard: stage 1/2 closure, source, and writer producer"
fi
if ! "${clean_environment[@]}" \
    "FN64_DISCOVER_NWXE_ROM=$FN64_DISCOVER_NWXE_ROM" \
    "FN64_BOOT_CONTEXT=$FN64_BOOT_CONTEXT" \
    "FN64_CLOSURE_AUDIT_DIR=$output_dir" \
    "FN64_SOURCE_FRONTIER_RECEIPT=$source_receipt" \
    "FN64_WRITER_CHANNEL_DENOMINATOR_RECEIPT=$writer_receipt" \
    "FN64_GUARD_MAX_RSS_MIB=$guard_max_rss_mib" \
    "FN64_GUARD_MIN_FREE_PERCENT=$guard_min_free_percent" \
    "CARGO_PROFILE_DEV_DEBUG=$cargo_profile_dev_debug" \
    "${frontier_image_environment[@]}" \
    "${frontier_command[@]}" >"$output_dir/frontier.log" 2>&1; then
    print -u2 -- "current static scorecard: static producer failed; private diagnostics retained"
    exit 1
fi
[[ -f $closure_receipt && -r $closure_receipt \
    && -f $source_receipt && -r $source_receipt \
    && -f $writer_receipt && -r $writer_receipt ]] || {
    print -u2 -- "current static scorecard: static producer did not produce all three fixed receipts"
    exit 1
}

typeset selected_writer_receipt=$writer_receipt
if (( full_writer_audit )); then
    [[ ! -e $writer_audit_dir && ! -L $writer_audit_dir ]] || {
        print -u2 -- "current static scorecard: writer-audit output unexpectedly exists"
        exit 1
    }
    print -- "current static scorecard: stage 2/3 full writer audit"
    typeset -a writer_arguments
    writer_arguments=(
        --rom $FN64_DISCOVER_NWXE_ROM
        --boot-context $FN64_BOOT_CONTEXT
        "${image_group_arguments[@]}"
        --output $writer_audit_dir
    )
    if (( build_timeout_seen )); then
        writer_arguments+=(--max-build-seconds $build_timeout)
    fi
    typeset -r writer_progress_log=$output_dir/writer-progress.log
    typeset -a writer_pipeline_status
    set +e
    "${clean_environment[@]}" \
        "FN64_GUARD_MAX_RSS_MIB=$selected_build_guard_max_rss_mib" \
        "FN64_GUARD_MIN_FREE_PERCENT=$guard_min_free_percent" \
        "CARGO_PROFILE_DEV_DEBUG=$cargo_profile_dev_debug" \
        "${writer_command[@]}" "${writer_arguments[@]}" \
        2>"$output_dir/writer-audit.log" | tee "$writer_progress_log"
    writer_pipeline_status=("${pipestatus[@]}")
    typeset -ri writer_status=$writer_pipeline_status[1]
    typeset -ri tee_status=$writer_pipeline_status[2]
    set -e
    if (( writer_status != 0 || tee_status != 0 )); then
        print -u2 -- "current static scorecard: full writer audit failed; private diagnostics retained"
        exit 1
    fi
    [[ -f $completed_writer_receipt && -r $completed_writer_receipt \
        && -f $writer_audit_dir/writer-audit.json \
        && -r $writer_audit_dir/writer-audit.json ]] || {
        print -u2 -- "current static scorecard: full writer audit did not produce both fixed receipts"
        exit 1
    }
    selected_writer_receipt=$completed_writer_receipt
fi

if (( full_writer_audit )); then
    print -- "current static scorecard: stage 3/3 scorecard aggregation"
else
    print -- "current static scorecard: stage 2/2 scorecard aggregation"
fi
typeset -a scorecard_arguments
scorecard_arguments=(
    --closure-audit $closure_receipt
    --source-frontier $source_receipt
    --writer-denominator $selected_writer_receipt
    --evidence-label current
    --ack-current-is-caller-attested
    --format json
)
if (( full_writer_audit )); then
    scorecard_arguments+=(--writer-audit $writer_audit_dir/writer-audit.json)
fi
if ! "${scorecard_command[@]}" "${scorecard_arguments[@]}" \
    >"$scorecard_receipt" 2>"$output_dir/scorecard.log"; then
    print -u2 -- "current static scorecard: aggregation failed; private diagnostics retained"
    exit 1
fi
[[ -s $scorecard_receipt ]] || {
    print -u2 -- "current static scorecard: aggregator produced an empty scorecard"
    exit 1
}

print -- "current static scorecard: completed; private receipts retained"
command cat "$scorecard_receipt" 2>/dev/null || {
    print -u2 -- "current static scorecard: cannot read the completed scorecard"
    exit 1
}
