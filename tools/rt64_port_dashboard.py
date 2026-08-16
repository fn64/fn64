#!/usr/bin/env python3
"""Render and validate fn64's RT64-port workflow dashboard.

Reads the strict manifest at docs/rt64-port-status.json (checked against
docs/rt64-port-status-schema.json) and renders three views from one source of
truth: a concise terminal table (--terminal), generated Markdown
(docs/RT64-PORT-DASHBOARD.md), and a self-contained responsive local HTML
dashboard (docs/rt64-port-dashboard.html). Dependency-free: standard library
only, so it runs anywhere Python 3 runs.

States are never inferred from code, tests, or file presence. The manifest is
the only place a ticket or milestone state may be asserted, and this tool
only mechanically checks and renders it.
"""

from __future__ import annotations

import argparse
from datetime import datetime
import html
import json
import os
import re
import socketserver
import sys
import tempfile
import http.server
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parent.parent
SCHEMA_PATH = ROOT / "docs/rt64-port-status-schema.json"
STATUS_PATH = ROOT / "docs/rt64-port-status.json"
MARKDOWN_PATH = ROOT / "docs/RT64-PORT-DASHBOARD.md"
HTML_PATH = ROOT / "docs/rt64-port-dashboard.html"

TIMESTAMP_RE = re.compile(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$")


class DashboardError(RuntimeError):
    """A schema, manifest, or generated-output error. Fails loud, never silently continues."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise DashboardError(message)


def load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        raise DashboardError(f"cannot read {path}: {error}") from error
    except json.JSONDecodeError as error:
        raise DashboardError(f"cannot parse {path}: {error}") from error


# --------------------------------------------------------------------------
# Validation
# --------------------------------------------------------------------------


def _require_bounded_str(value: Any, field: str, max_chars: int) -> None:
    require(isinstance(value, str) and value.strip() != "", f"{field}: must be a non-empty string")
    require(len(value) <= max_chars, f"{field}: exceeds {max_chars} chars ({len(value)})")


def _require_bounded_list(value: Any, field: str, max_items: int) -> list:
    require(isinstance(value, list), f"{field}: must be a list")
    require(len(value) <= max_items, f"{field}: exceeds {max_items} items ({len(value)})")
    return value


def check_path_privacy(schema: dict, value: str, field: str) -> None:
    privacy = schema["path_privacy"]
    for prefix in privacy["forbidden_prefixes"]:
        require(not value.startswith(prefix), f"{field}: private/absolute path prefix in {value!r}")
    for substring in privacy["forbidden_substrings"]:
        require(substring not in value, f"{field}: private-identity substring {substring!r} in {value!r}")
    require(not value.startswith("/"), f"{field}: absolute path {value!r} is not repo-relative")
    require(":" not in value.split("/")[0] if "/" in value else True, f"{field}: looks like a drive path: {value!r}")


def check_timestamp(value: Any, field: str) -> None:
    require(isinstance(value, str), f"{field}: timestamp must be a string")
    require(TIMESTAMP_RE.match(value) is not None, f"{field}: {value!r} is not an ISO-8601 UTC timestamp (YYYY-MM-DDTHH:MM:SSZ)")
    try:
        parsed = datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ")
    except ValueError as error:
        raise DashboardError(f"{field}: invalid UTC calendar timestamp {value!r}") from error
    require(2020 <= parsed.year <= 2100, f"{field}: implausible year in {value!r}")


def validate_schema_shape(schema: dict) -> None:
    require(schema.get("schema") == "fn64.rt64-port-status-schema.v1", "unsupported schema identity")
    for key in (
        "allowed_ticket_states", "allowed_milestone_states", "required_program_fields",
        "required_milestone_fields", "required_ticket_fields", "bounds", "id_pattern",
        "milestone_id_pattern", "timestamp_pattern", "path_privacy", "profile_enum",
        "effort_enum",
    ):
        require(key in schema, f"schema: missing top-level key {key!r}")


def validate_program(schema: dict, program: dict) -> None:
    bounds = schema["bounds"]
    for field in schema["required_program_fields"]:
        require(field in program, f"program: missing required field {field!r}")
    _require_bounded_str(program["goal"], "program.goal", bounds["max_goal_chars"])
    check_timestamp(program["updated_utc"], "program.updated_utc")
    require(
        program["program_state"] in {"PLANNED", "READY", "IN PROGRESS", "BLOCKED", "COMPLETE"},
        f"program.program_state: {program['program_state']!r} is not a published milestone-style state",
    )
    _require_bounded_str(program["branch"], "program.branch", bounds["max_text_field_chars"])
    _require_bounded_str(program["base_branch"], "program.base_branch", bounds["max_text_field_chars"])
    for doc in program.get("authority_docs", []):
        require(isinstance(doc, str) and (ROOT / doc).exists(), f"program.authority_docs: {doc!r} does not exist")


def validate_milestones(schema: dict, milestones: list) -> dict[str, dict]:
    bounds = schema["bounds"]
    id_re = re.compile(schema["milestone_id_pattern"])
    by_id: dict[str, dict] = {}
    require(isinstance(milestones, list) and milestones, "milestones: must be a non-empty list")
    require(len(milestones) <= bounds["max_array_items"], "milestones: exceeds max_array_items")
    for entry in milestones:
        require(isinstance(entry, dict), "milestones: each entry must be an object")
        for field in schema["required_milestone_fields"]:
            require(field in entry, f"milestone: missing required field {field!r}")
        milestone_id = entry["id"]
        require(id_re.match(milestone_id) is not None, f"milestone.id: {milestone_id!r} does not match {schema['milestone_id_pattern']!r}")
        require(milestone_id not in by_id, f"milestone.id: duplicate id {milestone_id!r}")
        _require_bounded_str(entry["title"], f"milestone[{milestone_id}].title", bounds["max_text_field_chars"])
        _require_bounded_str(entry["exit_headline"], f"milestone[{milestone_id}].exit_headline", bounds["max_text_field_chars"])
        require(
            entry["state"] in schema["allowed_milestone_states"],
            f"milestone[{milestone_id}].state: {entry['state']!r} not in {schema['allowed_milestone_states']}",
        )
        by_id[milestone_id] = entry
    return by_id


def validate_verification_runs(schema: dict, runs: Any, ticket_id: str) -> list[dict]:
    bounds = schema["bounds"]
    runs = _require_bounded_list(runs, f"ticket[{ticket_id}].verification_runs", bounds["max_verification_runs_items"])
    for index, run in enumerate(runs):
        where = f"ticket[{ticket_id}].verification_runs[{index}]"
        require(isinstance(run, dict), f"{where}: must be an object")
        for field in schema["verification_run_fields"]:
            require(field in run, f"{where}: missing required field {field!r}")
        _require_bounded_str(run["command"], f"{where}.command", bounds["max_text_field_chars"])
        require(type(run["clean_run_count"]) is int and run["clean_run_count"] >= 0, f"{where}.clean_run_count: must be a non-negative int")
        require(type(run["required_run_count"]) is int and run["required_run_count"] >= 0, f"{where}.required_run_count: must be a non-negative int")
        require(
            run["kind"] in schema["verification_run_kind_enum"],
            f"{where}.kind: {run['kind']!r} not in {schema['verification_run_kind_enum']}",
        )
    return runs


def validate_retrospective(schema: dict, retrospective: Any, ticket_id: str) -> None:
    bounds = schema["bounds"]
    where = f"ticket[{ticket_id}].retrospective"
    require(isinstance(retrospective, dict), f"{where}: must be an object")
    for field in schema["required_retrospective_fields"]:
        require(field in retrospective, f"{where}: missing required field {field!r}")
    _require_bounded_str(retrospective["friction"], f"{where}.friction", bounds["max_text_field_chars"])
    _require_bounded_str(retrospective["cause"], f"{where}.cause", bounds["max_text_field_chars"])
    _require_bounded_str(retrospective["prevention"], f"{where}.prevention", bounds["max_text_field_chars"])
    require(
        type(retrospective["estimated_minutes_saved"]) in {int, float}
        and retrospective["estimated_minutes_saved"] >= 0,
        f"{where}.estimated_minutes_saved: must be a non-negative number",
    )


def validate_tickets(schema: dict, tickets: list, milestone_ids: set[str]) -> list[dict]:
    bounds = schema["bounds"]
    id_re = re.compile(schema["id_pattern"])
    require(isinstance(tickets, list) and tickets, "tickets: must be a non-empty list")
    require(len(tickets) <= bounds["max_array_items"], "tickets: exceeds max_array_items")

    seen_ids: set[str] = set()
    for ticket in tickets:
        require(isinstance(ticket, dict), "tickets: each entry must be an object")
        for field in schema["required_ticket_fields"]:
            require(field in ticket, f"ticket: missing required field {field!r} (id={ticket.get('id')!r})")
        ticket_id = ticket["id"]
        require(
            bounds["min_ticket_id_len"] <= len(ticket_id) <= bounds["max_ticket_id_len"],
            f"ticket.id: {ticket_id!r} length out of bounds",
        )
        require(id_re.match(ticket_id) is not None, f"ticket.id: {ticket_id!r} does not match {schema['id_pattern']!r}")
        require(ticket_id not in seen_ids, f"ticket.id: duplicate id {ticket_id!r}")
        seen_ids.add(ticket_id)

        require(ticket["milestone"] in milestone_ids, f"ticket[{ticket_id}].milestone: {ticket['milestone']!r} is not a declared milestone id")
        _require_bounded_str(ticket["objective"], f"ticket[{ticket_id}].objective", bounds["max_text_field_chars"])
        require(ticket["profile"] in schema["profile_enum"], f"ticket[{ticket_id}].profile: {ticket['profile']!r} not in {schema['profile_enum']}")
        _require_bounded_str(ticket["model"], f"ticket[{ticket_id}].model", bounds["max_text_field_chars"])
        require(ticket["model"].strip().upper() not in {"TBD", "TODO", "UNKNOWN", "N/A"}, f"ticket[{ticket_id}].model: must name an exact model, not a placeholder")
        require(ticket["effort"] in schema["effort_enum"], f"ticket[{ticket_id}].effort: {ticket['effort']!r} not in {schema['effort_enum']}")
        _require_bounded_str(ticket["owner"], f"ticket[{ticket_id}].owner", bounds["max_text_field_chars"])
        _require_bounded_str(ticket["branch"], f"ticket[{ticket_id}].branch", bounds["max_text_field_chars"])
        _require_bounded_str(ticket["base_branch"], f"ticket[{ticket_id}].base_branch", bounds["max_text_field_chars"])

        writable_paths = _require_bounded_list(ticket["writable_paths"], f"ticket[{ticket_id}].writable_paths", bounds["max_writable_paths_items"])
        require(len(writable_paths) > 0, f"ticket[{ticket_id}].writable_paths: must name at least one path")
        for path_value in writable_paths:
            require(isinstance(path_value, str), f"ticket[{ticket_id}].writable_paths: entries must be strings")
            check_path_privacy(schema, path_value, f"ticket[{ticket_id}].writable_paths")

        dependencies = _require_bounded_list(ticket["dependencies"], f"ticket[{ticket_id}].dependencies", bounds["max_dependencies_items"])
        for dep in dependencies:
            require(isinstance(dep, str), f"ticket[{ticket_id}].dependencies: entries must be strings")

        require(
            ticket["state"] in schema["allowed_ticket_states"],
            f"ticket[{ticket_id}].state: {ticket['state']!r} not in {schema['allowed_ticket_states']}",
        )

        findings = _require_bounded_list(ticket["findings"], f"ticket[{ticket_id}].findings", bounds["max_findings_items"])
        for index, finding in enumerate(findings):
            _require_bounded_str(finding, f"ticket[{ticket_id}].findings[{index}]", bounds["max_text_field_chars"])

        verification_runs = validate_verification_runs(schema, ticket["verification_runs"], ticket_id)

        blocker = ticket["blocker"]
        require(blocker is None or isinstance(blocker, str), f"ticket[{ticket_id}].blocker: must be null or a string")
        if isinstance(blocker, str):
            _require_bounded_str(blocker, f"ticket[{ticket_id}].blocker", bounds["max_text_field_chars"])
        if ticket["state"] == "BLOCKED":
            require(bool(blocker and blocker.strip()), f"ticket[{ticket_id}]: BLOCKED state requires a non-empty blocker")
        else:
            require(blocker is None, f"ticket[{ticket_id}]: non-BLOCKED state must have blocker=null")

        _require_bounded_str(ticket["next_action"], f"ticket[{ticket_id}].next_action", bounds["max_text_field_chars"])

        check_timestamp(ticket["started_utc"], f"ticket[{ticket_id}].started_utc")
        check_timestamp(ticket["updated_utc"], f"ticket[{ticket_id}].updated_utc")
        require(
            ticket["updated_utc"] >= ticket["started_utc"],
            f"ticket[{ticket_id}]: updated_utc {ticket['updated_utc']!r} precedes started_utc {ticket['started_utc']!r}",
        )

        # Review/integration are evidence claims. A recorded command is not a
        # receipt until its declared reliability bar has actually been met.
        if ticket["state"] in {"READY_FOR_REVIEW", "INTEGRATED"}:
            require(verification_runs, f"ticket[{ticket_id}]: {ticket['state']} with no verification_runs is a false-completion shape")
            for index, run in enumerate(verification_runs):
                require(
                    run["required_run_count"] > 0
                    and run["clean_run_count"] >= run["required_run_count"],
                    f"ticket[{ticket_id}].verification_runs[{index}]: {ticket['state']} before the declared reliability bar",
                )

        external_issue = ticket.get("external_issue")
        if external_issue is not None:
            _require_bounded_str(external_issue, f"ticket[{ticket_id}].external_issue", bounds["max_text_field_chars"])
            require(
                re.fullmatch(r"https://github\.com/[^/\s]+/[^/\s]+/issues/[1-9][0-9]*", external_issue) is not None,
                f"ticket[{ticket_id}].external_issue: must be a canonical GitHub issue URL",
            )

        if "retrospective" in ticket:
            validate_retrospective(schema, ticket["retrospective"], ticket_id)

    return tickets


def _dependency_cycle_check(tickets: list[dict]) -> None:
    by_id = {ticket["id"]: ticket for ticket in tickets}
    for ticket in tickets:
        for dep in ticket["dependencies"]:
            require(dep in by_id, f"ticket[{ticket['id']}].dependencies: unknown dependency {dep!r}")

    WHITE, GRAY, BLACK = 0, 1, 2
    color = {ticket_id: WHITE for ticket_id in by_id}

    def visit(ticket_id: str, stack: list[str]) -> None:
        color[ticket_id] = GRAY
        for dep in by_id[ticket_id]["dependencies"]:
            if color[dep] == GRAY:
                cycle = " -> ".join(stack + [dep])
                raise DashboardError(f"dependency cycle detected: {cycle}")
            if color[dep] == WHITE:
                visit(dep, stack + [dep])
        color[ticket_id] = BLACK

    for ticket_id in by_id:
        if color[ticket_id] == WHITE:
            visit(ticket_id, [ticket_id])


def validate_manifest(schema: dict, manifest: dict) -> tuple[dict, dict[str, dict], list[dict]]:
    require(manifest.get("schema") == "fn64.rt64-port-status.v1", "manifest: unsupported schema identity")
    require(isinstance(manifest.get("program"), dict), "manifest: missing program object")
    validate_program(schema, manifest["program"])
    milestone_by_id = validate_milestones(schema, manifest.get("milestones", []))
    tickets = validate_tickets(schema, manifest.get("tickets", []), set(milestone_by_id))
    _dependency_cycle_check(tickets)
    return manifest["program"], milestone_by_id, tickets


def load_and_validate() -> tuple[dict, dict, dict[str, dict], list[dict]]:
    schema = load_json(SCHEMA_PATH)
    validate_schema_shape(schema)
    manifest = load_json(STATUS_PATH)
    program, milestones, tickets = validate_manifest(schema, manifest)
    return schema, program, milestones, tickets


# --------------------------------------------------------------------------
# Derived summaries (never inferred beyond what the manifest states)
# --------------------------------------------------------------------------


def milestone_progress(milestones: dict[str, dict], tickets: list[dict]) -> dict[str, dict]:
    progress: dict[str, dict] = {}
    for milestone_id, entry in milestones.items():
        related = [t for t in tickets if t["milestone"] == milestone_id]
        counts: dict[str, int] = {}
        for t in related:
            counts[t["state"]] = counts.get(t["state"], 0) + 1
        progress[milestone_id] = {
            "milestone_state": entry["state"],
            "ticket_count": len(related),
            "state_counts": counts,
        }
    return progress


WORKFLOW_QUEUE_STATES = ("RUNNING", "READY_FOR_REVIEW", "READY", "BLOCKED")


def reliability_status(ticket: dict) -> str:
    """Return a receipt status without promoting a ticket's declared state."""
    runs = ticket["verification_runs"]
    if not runs or any(run["required_run_count"] == 0 for run in runs):
        return "NOT RECORDED"
    if all(run["clean_run_count"] >= run["required_run_count"] for run in runs):
        return "MET"
    return "NOT MET"


def reliability_bars(ticket: dict) -> str:
    runs = ticket["verification_runs"]
    if not runs:
        return "NOT RECORDED"
    bars = ", ".join(
        f"{run['command']}: {run['clean_run_count']}/{run['required_run_count']} {run['kind']}"
        for run in runs
    )
    return f"{reliability_status(ticket)} ({bars})"


def dependency_states(ticket: dict, tickets_by_id: dict[str, dict]) -> str:
    if not ticket["dependencies"]:
        return "none"
    return ", ".join(
        f"{dependency}={tickets_by_id[dependency]['state']}"
        for dependency in ticket["dependencies"]
    )


def workflow_frontier(milestones: dict[str, dict], tickets: list[dict]) -> dict[str, Any]:
    """Build a deterministic workflow summary from declared manifest facts only."""
    tickets_by_id = {ticket["id"]: ticket for ticket in tickets}
    return {
        "complete_milestone_count": sum(entry["state"] == "COMPLETE" for entry in milestones.values()),
        "milestone_count": len(milestones),
        "integrated_ticket_count": sum(ticket["state"] == "INTEGRATED" for ticket in tickets),
        "ticket_count": len(tickets),
        "no_ticket_milestones": [
            milestone_id
            for milestone_id in milestones
            if not any(ticket["milestone"] == milestone_id for ticket in tickets)
        ],
        "queues": {
            state: [ticket for ticket in tickets if ticket["state"] == state]
            for state in WORKFLOW_QUEUE_STATES
        },
        "tickets_by_id": tickets_by_id,
        "retrospective_ticket_count": sum("retrospective" in ticket for ticket in tickets),
        "missing_retrospective_ids": [ticket["id"] for ticket in tickets if "retrospective" not in ticket],
    }


def workflow_ticket_line(ticket: dict, tickets_by_id: dict[str, dict]) -> str:
    line = (
        f"{ticket['id']} [{ticket['state']}] owner={ticket['owner']} "
        f"branch={ticket['branch']} -> {ticket['base_branch']} "
        f"deps={dependency_states(ticket, tickets_by_id)} "
        f"reliability={reliability_bars(ticket)} next={ticket['next_action']}"
    )
    if ticket["blocker"]:
        line += f" blocker={ticket['blocker']}"
    return line


# --------------------------------------------------------------------------
# Terminal view
# --------------------------------------------------------------------------


def render_terminal(program: dict, milestones: dict[str, dict], tickets: list[dict]) -> str:
    lines: list[str] = []
    lines.append("RT64 PORT DASHBOARD")
    lines.append(f"goal: {program['goal']}")
    lines.append(f"program state: {program['program_state']}  branch: {program['branch']} -> {program['base_branch']}")
    lines.append("")
    frontier = workflow_frontier(milestones, tickets)
    lines.append("WORKFLOW FRONTIER")
    lines.append(f"  COMPLETE milestones: {frontier['complete_milestone_count']}/{frontier['milestone_count']}")
    lines.append(
        f"  INTEGRATED / recorded tickets: {frontier['integrated_ticket_count']}/{frontier['ticket_count']} "
        "(recorded ticket scope, not a percent-to-goal)"
    )
    no_tickets = ", ".join(frontier["no_ticket_milestones"]) or "none"
    lines.append(f"  milestones with no tickets: {no_tickets}")
    missing = ", ".join(frontier["missing_retrospective_ids"]) or "none"
    lines.append(
        f"  retrospective coverage: {frontier['retrospective_ticket_count']}/{frontier['ticket_count']}; "
        f"missing: {missing}"
    )
    for state in WORKFLOW_QUEUE_STATES:
        label = "READY_FOR_REVIEW (awaiting independent review only)" if state == "READY_FOR_REVIEW" else state
        queue = frontier["queues"][state]
        lines.append(f"  {label}: {len(queue)}")
        for ticket in queue:
            lines.append(f"    {workflow_ticket_line(ticket, frontier['tickets_by_id'])}")
    lines.append("")
    lines.append("MILESTONES")
    progress = milestone_progress(milestones, tickets)
    for milestone_id, entry in milestones.items():
        p = progress[milestone_id]
        counts = ", ".join(f"{state}:{count}" for state, count in sorted(p["state_counts"].items())) or "no tickets"
        lines.append(f"  {milestone_id:5} [{entry['state']:11}] {entry['title']}")
        lines.append(f"        tickets: {counts}")
    lines.append("")
    lines.append("TICKETS")
    for ticket in tickets:
        lines.append(
            f"  {ticket['id']:8} [{ticket['state']:16}] {ticket['milestone']:5} "
            f"{ticket['profile']}/{ticket['effort']:6} {ticket['model']}  owner={ticket['owner']}"
        )
        lines.append(f"           {ticket['objective']}")
        lines.append(f"           deps={dependency_states(ticket, frontier['tickets_by_id'])}  reliability={reliability_bars(ticket)}")
        if ticket["blocker"]:
            lines.append(f"           BLOCKER: {ticket['blocker']}")
        lines.append(f"           next: {ticket['next_action']}")
    return "\n".join(lines) + "\n"


# --------------------------------------------------------------------------
# Markdown view
# --------------------------------------------------------------------------


def render_markdown(program: dict, milestones: dict[str, dict], tickets: list[dict]) -> str:
    frontier = workflow_frontier(milestones, tickets)
    lines: list[str] = [
        "# RT64 port workflow dashboard",
        "",
        "<!-- Generated by tools/rt64_port_dashboard.py from docs/rt64-port-status.json. -->",
        "<!-- Edit the JSON manifest, then regenerate this file; do not edit this report directly. -->",
        "",
        "## Program",
        "",
        f"**Goal:** {program['goal']}",
        "",
        f"| field | value |",
        f"|---|---|",
        f"| state | `{program['program_state']}` |",
        f"| branch | `{program['branch']}` -> `{program['base_branch']}` |",
        "",
        "## Workflow frontier",
        "",
        f"- **COMPLETE milestones:** {frontier['complete_milestone_count']}/{frontier['milestone_count']}",
        f"- **INTEGRATED / recorded tickets:** {frontier['integrated_ticket_count']}/{frontier['ticket_count']} — recorded ticket scope, **not a percent-to-goal**.",
        f"- **Milestones with no tickets:** {', '.join(f'`{milestone_id}`' for milestone_id in frontier['no_ticket_milestones']) or 'none'}",
        f"- **Retrospective coverage:** {frontier['retrospective_ticket_count']}/{frontier['ticket_count']}; missing: {', '.join(f'`{ticket_id}`' for ticket_id in frontier['missing_retrospective_ids']) or 'none'}",
        "",
    ]
    for state in WORKFLOW_QUEUE_STATES:
        label = "READY_FOR_REVIEW — awaiting independent review only" if state == "READY_FOR_REVIEW" else state
        lines.extend([f"### {label} ({len(frontier['queues'][state])})", ""])
        if not frontier["queues"][state]:
            lines.extend(["None.", ""])
            continue
        for ticket in frontier["queues"][state]:
            blocker = f"; blocker: {ticket['blocker']}" if ticket["blocker"] else ""
            lines.extend([
                f"- `{ticket['id']}` — owner: {ticket['owner']}; branch: `{ticket['branch']}` -> `{ticket['base_branch']}`; dependencies: {dependency_states(ticket, frontier['tickets_by_id'])}; reliability: **{reliability_bars(ticket)}**; next: {ticket['next_action']}{blocker}",
                "",
            ])
    lines.extend([
        "## Milestones",
        "",
        "| ID | state | title | tickets |",
        "|---|---|---|---|",
    ])
    progress = milestone_progress(milestones, tickets)
    for milestone_id, entry in milestones.items():
        p = progress[milestone_id]
        counts = ", ".join(f"{state}:{count}" for state, count in sorted(p["state_counts"].items())) or "none"
        lines.append(f"| `{milestone_id}` | `{entry['state']}` | {entry['title']} | {counts} |")
    lines.extend(["", "Each milestone's exit gate (from `docs/RENDER-WGPU-PORT-PLAN.md`):", ""])
    for milestone_id, entry in milestones.items():
        lines.append(f"- **{milestone_id}:** {entry['exit_headline']}")
    lines.extend(["", "## Tickets", ""])
    for ticket in tickets:
        lines.extend([
            f"### `{ticket['id']}` -- {ticket['objective']}",
            "",
            f"| field | value |",
            f"|---|---|",
            f"| milestone | `{ticket['milestone']}` |",
            f"| state | `{ticket['state']}` |",
            f"| profile / effort / model | `{ticket['profile']}` / `{ticket['effort']}` / {ticket['model']} |",
            f"| owner | {ticket['owner']} |",
            f"| branch | `{ticket['branch']}` -> `{ticket['base_branch']}` |",
            f"| dependencies | {dependency_states(ticket, frontier['tickets_by_id'])} |",
            f"| writable paths | {', '.join(f'`{p}`' for p in ticket['writable_paths'])} |",
            f"| reliability | **{reliability_bars(ticket)}** |",
            "",
        ])
        if ticket["findings"]:
            lines.append("**Findings:**")
            lines.append("")
            for finding in ticket["findings"]:
                lines.append(f"- {finding}")
            lines.append("")
        if ticket["verification_runs"]:
            lines.append("**Verification:**")
            lines.append("")
            lines.append("| command | clean runs | required | kind |")
            lines.append("|---|---|---|---|")
            for run in ticket["verification_runs"]:
                lines.append(f"| `{run['command']}` | {run['clean_run_count']} | {run['required_run_count']} | {run['kind']} |")
            lines.append("")
        if ticket["blocker"]:
            lines.append(f"**Blocker:** {ticket['blocker']}")
            lines.append("")
        lines.append(f"**Next action:** {ticket['next_action']}")
        lines.append("")
        if "retrospective" in ticket:
            r = ticket["retrospective"]
            lines.extend([
                "**Retrospective:**",
                "",
                f"- Friction: {r['friction']}",
                f"- Cause: {r['cause']}",
                f"- Prevention: {r['prevention']}",
                f"- Estimated minutes saved: {r['estimated_minutes_saved']}",
                "",
            ])
    lines.extend([
        "## Regenerating",
        "",
        "```sh",
        "python3 tools/rt64_port_dashboard.py --check",
        "python3 tools/rt64_port_dashboard.py --write",
        "python3 tools/rt64_port_dashboard.py --terminal",
        "python3 tools/rt64_port_dashboard.py --serve",
        "```",
        "",
        "`--check` validates the manifest against the schema and confirms the generated",
        "Markdown and HTML are not stale. `--write` regenerates both atomically. `--serve`",
        "starts a read-only local HTTP server bound to `127.0.0.1` only, serving the",
        "generated HTML dashboard.",
        "",
    ])
    return "\n".join(lines).rstrip() + "\n"


# --------------------------------------------------------------------------
# HTML view (self-contained, no external assets, responsive)
# --------------------------------------------------------------------------


_STATE_CLASS = {
    "READY": "state-ready",
    "RUNNING": "state-running",
    "READY_FOR_REVIEW": "state-review",
    "BLOCKED": "state-blocked",
    "REJECTED": "state-rejected",
    "INTEGRATED": "state-integrated",
    "PLANNED": "state-planned",
    "IN PROGRESS": "state-running",
    "COMPLETE": "state-integrated",
}


def _e(value: Any) -> str:
    return html.escape(str(value), quote=True)


def render_html(program: dict, milestones: dict[str, dict], tickets: list[dict]) -> str:
    progress = milestone_progress(milestones, tickets)
    frontier = workflow_frontier(milestones, tickets)

    milestone_rows = []
    for milestone_id, entry in milestones.items():
        p = progress[milestone_id]
        counts = ", ".join(f"{state}:{count}" for state, count in sorted(p["state_counts"].items())) or "no tickets"
        state_class = _STATE_CLASS.get(entry["state"], "")
        milestone_rows.append(
            f"<tr><td><code>{_e(milestone_id)}</code></td>"
            f"<td><span class=\"badge {state_class}\">{_e(entry['state'])}</span></td>"
            f"<td>{_e(entry['title'])}</td>"
            f"<td>{_e(entry['exit_headline'])}</td>"
            f"<td>{_e(counts)}</td></tr>"
        )

    ticket_cards = []
    for ticket in tickets:
        state_class = _STATE_CLASS.get(ticket["state"], "")
        findings_html = "".join(f"<li>{_e(f)}</li>" for f in ticket["findings"]) or "<li class=\"muted\">none recorded</li>"
        runs_rows = "".join(
            f"<tr><td><code>{_e(run['command'])}</code></td><td>{run['clean_run_count']}</td>"
            f"<td>{run['required_run_count']}</td><td>{_e(run['kind'])}</td></tr>"
            for run in ticket["verification_runs"]
        ) or "<tr><td colspan=\"4\" class=\"muted\">no verification runs recorded</td></tr>"
        deps_html = _e(dependency_states(ticket, frontier["tickets_by_id"]))
        paths_html = ", ".join(f"<code>{_e(p)}</code>" for p in ticket["writable_paths"])
        blocker_html = (
            f"  <p class=\"blocker\"><strong>Blocker:</strong> {_e(ticket['blocker'])}</p>\n"
            if ticket["blocker"] else ""
        )
        retrospective_html = ""
        if "retrospective" in ticket:
            r = ticket["retrospective"]
            retrospective_html = (
                "  <details><summary>Retrospective</summary><ul>"
                f"<li><strong>Friction:</strong> {_e(r['friction'])}</li>"
                f"<li><strong>Cause:</strong> {_e(r['cause'])}</li>"
                f"<li><strong>Prevention:</strong> {_e(r['prevention'])}</li>"
                f"<li><strong>Est. minutes saved:</strong> {_e(r['estimated_minutes_saved'])}</li>"
                "</ul></details>\n"
            )
        ticket_cards.append(f"""
<article class="ticket" data-state="{_e(ticket['state'])}" data-milestone="{_e(ticket['milestone'])}">
  <header>
    <h3><code>{_e(ticket['id'])}</code> <span class="badge {state_class}">{_e(ticket['state'])}</span></h3>
    <p class="objective">{_e(ticket['objective'])}</p>
  </header>
  <dl class="meta">
    <dt>Milestone</dt><dd><code>{_e(ticket['milestone'])}</code></dd>
    <dt>Profile / effort / model</dt><dd>{_e(ticket['profile'])} / {_e(ticket['effort'])} / {_e(ticket['model'])}</dd>
    <dt>Owner</dt><dd>{_e(ticket['owner'])}</dd>
    <dt>Branch</dt><dd><code>{_e(ticket['branch'])}</code> &rarr; <code>{_e(ticket['base_branch'])}</code></dd>
    <dt>Dependencies</dt><dd>{deps_html}</dd>
    <dt>Writable paths</dt><dd>{paths_html}</dd>
    <dt>Reliability</dt><dd>{_e(reliability_bars(ticket))}</dd>
  </dl>
{blocker_html}\
  <p><strong>Next action:</strong> {_e(ticket['next_action'])}</p>
  <details open><summary>Findings</summary><ul>{findings_html}</ul></details>
  <details><summary>Verification runs</summary>
    <table><thead><tr><th>command</th><th>clean</th><th>required</th><th>kind</th></tr></thead>
    <tbody>{runs_rows}</tbody></table>
  </details>
{retrospective_html}\
</article>""")

    queue_sections = []
    for state in WORKFLOW_QUEUE_STATES:
        label = "READY_FOR_REVIEW — awaiting independent review only" if state == "READY_FOR_REVIEW" else state
        queue_entries = "".join(
            f"<li><code>{_e(ticket['id'])}</code> — owner: {_e(ticket['owner'])}; "
            f"branch: <code>{_e(ticket['branch'])}</code> &rarr; <code>{_e(ticket['base_branch'])}</code>; "
            f"dependencies: {_e(dependency_states(ticket, frontier['tickets_by_id']))}; "
            f"reliability: <strong>{_e(reliability_bars(ticket))}</strong>; "
            f"next: {_e(ticket['next_action'])}"
            f"{'; blocker: ' + _e(ticket['blocker']) if ticket['blocker'] else ''}</li>"
            for ticket in frontier["queues"][state]
        ) or '<li class="muted">none</li>'
        queue_sections.append(
            f"<section class=\"queue\"><h3>{_e(label)} ({len(frontier['queues'][state])})</h3>"
            f"<ul>{queue_entries}</ul></section>"
        )
    no_ticket_milestones = ", ".join(frontier["no_ticket_milestones"]) or "none"
    missing_retrospectives = ", ".join(frontier["missing_retrospective_ids"]) or "none"

    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>RT64 port workflow dashboard</title>
<style>
  :root {{ color-scheme: light dark; }}
  * {{ box-sizing: border-box; }}
  body {{
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif;
    margin: 0; padding: 1.25rem; line-height: 1.5;
    background: light-dark(#f7f7f8, #14161a); color: light-dark(#1a1a1a, #e8e8e8);
  }}
  h1 {{ font-size: 1.4rem; margin-bottom: 0.25rem; }}
  h2 {{ font-size: 1.15rem; margin-top: 2rem; }}
  code {{ font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: 0.9em; }}
  .goal {{ max-width: 70ch; opacity: 0.85; }}
  .frontier {{ border: 1px solid light-dark(#ddd, #333); border-radius: 8px; padding: 0.9rem 1rem; background: light-dark(#fff, #1c1f24); }}
  .frontier p {{ margin: 0.3rem 0; }}
  .warning {{ color: #9a6700; font-weight: 600; }}
  .queues {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(290px, 1fr)); gap: 0.75rem; }}
  .queue {{ border-top: 1px solid light-dark(#ddd, #333); }}
  .queue h3 {{ font-size: 0.95rem; margin: 0.6rem 0 0.2rem; }}
  .badge {{
    display: inline-block; padding: 0.1rem 0.5rem; border-radius: 999px;
    font-size: 0.75rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.02em;
    border: 1px solid currentColor;
  }}
  .state-ready {{ color: #0969da; }}
  .state-running {{ color: #9a6700; }}
  .state-review {{ color: #8250df; }}
  .state-blocked {{ color: #cf222e; }}
  .state-rejected {{ color: #6e7781; }}
  .state-integrated {{ color: #1a7f37; }}
  .state-planned {{ color: #6e7781; }}
  table {{ border-collapse: collapse; width: 100%; margin: 0.5rem 0 1rem; }}
  th, td {{ text-align: left; padding: 0.35rem 0.5rem; border-bottom: 1px solid light-dark(#ddd, #333); vertical-align: top; }}
  th {{ font-size: 0.8rem; text-transform: uppercase; opacity: 0.7; }}
  .muted {{ opacity: 0.6; font-style: italic; }}
  .tickets {{ display: grid; grid-template-columns: repeat(auto-fill, minmax(340px, 1fr)); gap: 1rem; }}
  .ticket {{
    border: 1px solid light-dark(#ddd, #333); border-radius: 8px; padding: 0.9rem 1rem;
    background: light-dark(#fff, #1c1f24);
  }}
  .ticket header h3 {{ margin: 0 0 0.25rem; font-size: 1rem; display: flex; align-items: center; gap: 0.5rem; }}
  .objective {{ margin: 0 0 0.5rem; }}
  dl.meta {{ display: grid; grid-template-columns: auto 1fr; gap: 0.15rem 0.6rem; margin: 0.5rem 0; font-size: 0.9rem; }}
  dl.meta dt {{ opacity: 0.65; }}
  dl.meta dd {{ margin: 0; }}
  .blocker {{ color: #cf222e; }}
  details summary {{ cursor: pointer; font-weight: 600; margin-top: 0.5rem; }}
  ul {{ margin: 0.3rem 0; padding-left: 1.2rem; }}
  .filters {{ margin: 0.75rem 0; display: flex; gap: 0.5rem; flex-wrap: wrap; }}
  .filters select {{ padding: 0.3rem 0.5rem; }}
  @media (max-width: 480px) {{
    .tickets {{ grid-template-columns: 1fr; }}
    dl.meta {{ grid-template-columns: 1fr; }}
  }}
</style>
</head>
<body>
<h1>RT64 port workflow dashboard</h1>
<p class="goal">{_e(program['goal'])}</p>
<p>
  <span class="badge {_STATE_CLASS.get(program['program_state'], '')}">{_e(program['program_state'])}</span>
  <code>{_e(program['branch'])}</code> &rarr; <code>{_e(program['base_branch'])}</code>
</p>

<section class="frontier">
<h2>Workflow frontier</h2>
<p><strong>COMPLETE milestones:</strong> {frontier['complete_milestone_count']}/{frontier['milestone_count']}</p>
<p><strong>INTEGRATED / recorded tickets:</strong> {frontier['integrated_ticket_count']}/{frontier['ticket_count']}</p>
<p class="warning">Recorded ticket scope, not a percent-to-goal.</p>
<p><strong>Milestones with no tickets:</strong> {_e(no_ticket_milestones)}</p>
<p><strong>Retrospective coverage:</strong> {frontier['retrospective_ticket_count']}/{frontier['ticket_count']}; missing: {_e(missing_retrospectives)}</p>
<div class="queues">
{"".join(queue_sections)}
</div>
</section>

<h2>Milestones</h2>
<table>
  <thead><tr><th>ID</th><th>state</th><th>title</th><th>exit headline</th><th>tickets</th></tr></thead>
  <tbody>{"".join(milestone_rows)}</tbody>
</table>

<h2>Tickets</h2>
<div class="filters">
  <label>State
    <select id="state-filter">
      <option value="">all</option>
      {"".join(f'<option value="{_e(s)}">{_e(s)}</option>' for s in sorted({t["state"] for t in tickets}))}
    </select>
  </label>
  <label>Milestone
    <select id="milestone-filter">
      <option value="">all</option>
      {"".join(f'<option value="{_e(m)}">{_e(m)}</option>' for m in milestones.keys())}
    </select>
  </label>
</div>
<div class="tickets" id="ticket-list">
{"".join(ticket_cards)}
</div>

<script>
  const stateFilter = document.getElementById("state-filter");
  const milestoneFilter = document.getElementById("milestone-filter");
  const cards = Array.from(document.querySelectorAll(".ticket"));
  function applyFilters() {{
    const s = stateFilter.value;
    const m = milestoneFilter.value;
    for (const card of cards) {{
      const showState = !s || card.dataset.state === s;
      const showMilestone = !m || card.dataset.milestone === m;
      card.style.display = (showState && showMilestone) ? "" : "none";
    }}
  }}
  stateFilter.addEventListener("change", applyFilters);
  milestoneFilter.addEventListener("change", applyFilters);
</script>
</body>
</html>
"""


# --------------------------------------------------------------------------
# Atomic writes
# --------------------------------------------------------------------------


def atomic_write(path: Path, content: str) -> None:
    fd, tmp_name = tempfile.mkstemp(dir=str(path.parent), prefix=f".{path.name}.", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(content)
        os.replace(tmp_name, path)
    except BaseException:
        try:
            os.unlink(tmp_name)
        except OSError:
            pass
        raise


# --------------------------------------------------------------------------
# --serve: loopback-only, read-only local HTTP server
# --------------------------------------------------------------------------


class _LoopbackHTTPServer(socketserver.TCPServer):
    allow_reuse_address = True


def serve(host: str, port: int) -> None:
    require(host in ("127.0.0.1", "localhost", "::1"), f"--serve refuses non-loopback host {host!r}")
    directory = str(HTML_PATH.parent)

    class Handler(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *args: Any, **kwargs: Any) -> None:
            super().__init__(*args, directory=directory, **kwargs)

        def do_POST(self) -> None:  # noqa: N802 - stdlib method name
            self.send_error(405, "read-only dashboard server")

        def do_PUT(self) -> None:  # noqa: N802
            self.send_error(405, "read-only dashboard server")

        def do_DELETE(self) -> None:  # noqa: N802
            self.send_error(405, "read-only dashboard server")

        def log_message(self, format: str, *args: Any) -> None:  # noqa: A002
            sys.stderr.write(f"rt64-port-dashboard: {self.address_string()} {format % args}\n")

    with _LoopbackHTTPServer((host, port), Handler) as httpd:
        bound_port = httpd.server_address[1]
        print(f"rt64-port-dashboard: serving {HTML_PATH.name} at http://{host}:{bound_port}/{HTML_PATH.name} (loopback only, read-only)")
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            pass


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="validate the manifest and confirm generated outputs are not stale")
    parser.add_argument("--write", action="store_true", help="regenerate Markdown and HTML outputs atomically")
    parser.add_argument("--terminal", action="store_true", help="print the concise terminal view")
    parser.add_argument("--serve", action="store_true", help="serve the generated HTML dashboard on loopback only")
    parser.add_argument("--host", default="127.0.0.1", help="bind host for --serve (loopback only; default 127.0.0.1)")
    parser.add_argument("--port", type=int, default=8765, help="bind port for --serve (default 8765)")
    arguments = parser.parse_args()

    if not any([arguments.check, arguments.write, arguments.terminal, arguments.serve]):
        arguments.terminal = True

    try:
        schema, program, milestones, tickets = load_and_validate()
    except DashboardError as error:
        print(f"rt64-port-dashboard: {error}", file=sys.stderr)
        return 1

    if arguments.terminal:
        sys.stdout.write(render_terminal(program, milestones, tickets))

    if arguments.write:
        atomic_write(MARKDOWN_PATH, render_markdown(program, milestones, tickets))
        atomic_write(HTML_PATH, render_html(program, milestones, tickets))
        print(f"rt64-port-dashboard: wrote {MARKDOWN_PATH} and {HTML_PATH}")

    if arguments.check:
        expected_markdown = render_markdown(program, milestones, tickets)
        expected_html = render_html(program, milestones, tickets)
        try:
            require(MARKDOWN_PATH.is_file(), f"generated report is missing: {MARKDOWN_PATH}")
            require(MARKDOWN_PATH.read_text(encoding="utf-8") == expected_markdown, f"{MARKDOWN_PATH} is stale; run with --write")
            require(HTML_PATH.is_file(), f"generated report is missing: {HTML_PATH}")
            require(HTML_PATH.read_text(encoding="utf-8") == expected_html, f"{HTML_PATH} is stale; run with --write")
        except DashboardError as error:
            print(f"rt64-port-dashboard: {error}", file=sys.stderr)
            return 1
        print("rt64-port-dashboard: clean")

    if arguments.serve:
        serve(arguments.host, arguments.port)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
