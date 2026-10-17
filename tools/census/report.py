#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Aggregate census JSONs into docs/m0/opcode-census.md + CSVs.
usage: report.py OUT_DIR(with index.tsv + *.json) DOCS_DIR DYNARMIC_SRC DYNARMIC_COMMIT
"""
import csv
import json
import os
import re
import sys
from collections import Counter, defaultdict

out, docs, dynroot, dyncommit = sys.argv[1:5]
VER = {"x174": "1.7.4", "x175": "1.7.5", "x176": "1.7.6"}
idx = []
for ln in open(os.path.join(out, "index.tsv")):
    d, lib, h = ln.rstrip("\n").split("\t")
    idx.append((VER[[k for k in VER if "/" + k + "/" in d][0]], lib, h))
J = {h: json.load(open(os.path.join(out, h + ".json"))) for h in {h for _, _, h in idx}}
versions = sorted(VER.values())
libs = sorted({l for _, l, _ in idx})
by = {(v, l): J[h] for v, l, h in idx}

# ---------------------------------------------------------------- totals
tot_isa = {v: Counter() for v in versions}
tot_cls = {v: Counter() for v in versions}
for (v, l), d in by.items():
    tot_isa[v].update(d["insn_total"])
    for isa, cls, m, n in d["mnem"]:
        tot_cls[v][cls] += n
classes = ["A32", "T16", "T32", "VFP", "NEON", "CRYPTO", "COPROC", "SYSTEM", "UNDECODED"]

mn = defaultdict(lambda: {v: 0 for v in versions})
mn_libs = defaultdict(set)
for (v, l), d in by.items():
    for isa, cls, m, n in d["mnem"]:
        k = (isa, cls, m)
        mn[k][v] += n
        mn_libs[k].add(l)

with open(os.path.join(docs, "opcode-census.csv"), "w", newline="") as f:
    w = csv.writer(f)
    w.writerow(["isa", "class", "mnemonic"] + ["count_" + v for v in versions] + ["libs"])
    for k in sorted(mn, key=lambda k: (-mn[k]["1.7.6"], k)):
        w.writerow(list(k) + [mn[k][v] for v in versions] + [len(mn_libs[k])])

with open(os.path.join(docs, "opcode-census-libs.csv"), "w", newline="") as f:
    w = csv.writer(f)
    w.writerow(["version", "lib", "bytes", "function_starts", "A32_insns", "T16_insns", "T32_insns", "data_bytes",
                "zero_hi16_words", "pad_bytes", "undecoded_by_llvm", "no_dynarmic_decoder"] + ["start_src_" + s for s in
               ("dynsym", "exidx", "objc", "reloc", "initarr")] + ["mode_from_bit0", "mode_heuristic_a", "mode_heuristic_t", "mode_conflict"])
    for (v, l) in sorted(by):
        d = by[(v, l)]
        ds, ms, ss, it = d["data_stats"], d["mode_stats"], d["start_sources"], d["insn_total"]
        w.writerow([v, l, d["size"], d["starts"], it.get("A", 0), it.get("T16", 0), it.get("T32", 0), ds.get("data_bytes", 0),
                    ds.get("zero_hi16_words", 0), ds.get("pad_bytes", 0), len(d["unknown"]), len(d["unmatched"])] +
                   [ss.get(s, 0) for s in ("dynsym", "exidx", "objc", "reloc", "initarr")] +
                   [ms.get("mode_from_bit0", 0), ms.get("mode_heuristic_a", 0), ms.get("mode_heuristic_t", 0), ms.get("mode_conflict", 0)])

# ---------------------------------------------------------------- dynarmic diff
dyn = defaultdict(lambda: {"n": {v: 0 for v in versions}, "status": "OK", "detail": "", "name": "", "libs": set(),
                           "funcs": Counter(), "ex": "", "mn": Counter()})
for (v, l), d in by.items():
    for fn, n in d["dyn_all"].items():
        dyn[fn]["n"][v] += n
    for fn, x in d["dyn"].items():
        e = dyn[fn]
        e["status"], e["detail"], e["name"], e["ex"] = x["status"], x["detail"], x["name"], x["example"]
        e["libs"].add(l)
        if v == "1.7.6":
            for f_, c in x["funcs"]:
                e["funcs"][(l, f_)] += c
            for m_, c in x.get("mnems", []):
                e["mn"][m_] += c
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dynarmic_tables as dt  # noqa: E402
tabs = dt.Tables(dynroot)
handlers = dt.analyse_handlers(dynroot)
for fn, e in dyn.items():
    st = handlers.get(fn, ("OK", "", ""))
    e["status"], e["detail"] = st[0], st[1]
    e["name"] = tabs.by_fn[fn].name if fn in tabs.by_fn else ""
with open(os.path.join(docs, "opcode-census-dynarmic.csv"), "w", newline="") as f:
    w = csv.writer(f)
    w.writerow(["matcher", "name", "handler_status", "handler_special_call"] + ["count_" + v for v in versions] + ["top_functions_1.7.6"])
    for fn in sorted(dyn, key=lambda k: -dyn[k]["n"]["1.7.6"]):
        e = dyn[fn]
        w.writerow([fn, e["name"], e["status"], e["detail"]] + [e["n"][v] for v in versions] +
                   ["; ".join("%s:%s=%d" % (l, f_, c) for (l, f_), c in e["funcs"].most_common(8))])

# unmatched (no decoder entry in dynarmic)
unm = defaultdict(lambda: {"n": 0, "libs": Counter(), "funcs": Counter(), "encs": Counter()})
for (v, l), d in by.items():
    if v != "1.7.6":
        continue
    for isa, a, f_, bm, e_ in d["unmatched"]:
        k = (isa, bm)
        unm[k]["n"] += 1
        unm[k]["libs"][l] += 1
        unm[k]["funcs"][(l, f_)] += 1
        unm[k]["encs"][e_] += 1
unk = defaultdict(lambda: {"n": 0, "libs": Counter(), "funcs": Counter(), "encs": Counter()})
for (v, l), d in by.items():
    if v != "1.7.6":
        continue
    for a, f_, isa, e_ in d["unknown"]:
        k = isa
        unk[k]["n"] += 1
        unk[k]["libs"][l] += 1
        unk[k]["funcs"][(l, f_)] += 1
        unk[k]["encs"][e_] += 1
with open(os.path.join(docs, "opcode-census-unsupported.csv"), "w", newline="") as f:
    w = csv.writer(f)
    w.writerow(["version", "lib", "kind", "isa", "address", "function", "mnemonic_or_matcher"])
    for (v, l), d in sorted(by.items()):
        for isa, a, f_, bm, e_ in d["unmatched"]:
            w.writerow([v, l, "no_dynarmic_decoder", isa, a, f_, bm])
        for a, f_, isa, e_ in d["unknown"]:
            w.writerow([v, l, "llvm_undecodable", isa, a, f_, "<undecoded>"])

# odd / privileged
odd = defaultdict(lambda: {"n": {v: 0 for v in versions}, "libs": set(), "funcs": Counter(), "enc": ""})


def norm(t):
    t = re.sub(r"\s+", " ", t.strip())
    return t
for (v, l), d in by.items():
    for a, f_, isa, t, e_ in d["odd"]:
        t = norm(t)
        mnm = t.split()[0]
        key = t if re.match(r"^(mrc|mcr|svc|udf|bkpt|swp|cps|msr|mrs|vmrs|vmsr)", mnm) else mnm
        o = odd[key]
        o["n"][v] += 1
        o["libs"].add(l)
        if v == "1.7.6":
            o["funcs"][(l, f_)] += 1
        o["enc"] = e_
with open(os.path.join(docs, "opcode-census-odd.csv"), "w", newline="") as f:
    w = csv.writer(f)
    w.writerow(["instruction"] + ["count_" + v for v in versions] + ["libs", "top_functions_1.7.6"])
    for k in sorted(odd, key=lambda k: -odd[k]["n"]["1.7.6"]):
        o = odd[k]
        w.writerow([k] + [o["n"][v] for v in versions] + [";".join(sorted(o["libs"])),
                                                          "; ".join("%s:%s=%d" % (l, f_, c) for (l, f_), c in o["funcs"].most_common(6))])

# ---------------------------------------------------------------- markdown
V = "1.7.6"
L = []
P = L.append


def tbl(h, rows):
    P("| " + " | ".join(h) + " |")
    P("|" + "|".join("---" for _ in h) + "|")
    for r in rows:
        P("| " + " | ".join(str(x) for x in r) + " |")
    P("")


P("# M0 opcode census (symbol-bounded)\n")
P("Generated by `tools/census/` (census.py, run_all.sh, report.py). Dynarmic reference: `azahar-emu/dynarmic` @ `%s` (shallow clone, read-only)." % dyncommit)
P("Inputs: `lib/armeabi-v7a/*.so` (25 libs each) of The Blockheads 1.7.4, 1.7.5 and 1.7.6 APKs. Nothing from the APKs is stored in the repo.\n")
nstarts = sum(by[(V, l)]["starts"] for l in libs)
P("## 1. Headline\n")
t = tot_isa[V]
btt = sum(by[(V, l)].get("branch_check", {}).get("branches_to_text", 0) for l in libs)
bid = sum(by[(V, l)].get("branch_check", {}).get("branch_into_data", 0) for l in libs)
nthumb = sum(by[(V, l)].get("thumb_starts", 0) for l in libs)
P("- Instructions decoded (1.7.6, 25 libs, symbol-bounded): **%s** (ARM A32 %s, Thumb T16 %s, Thumb T32 %s)." % (
    format(sum(t.values()), ","), format(t.get("A", 0), ","), format(t.get("T16", 0), ","), format(t.get("T32", 0), ",")))
P("- Thumb function starts / Thumb instructions in any of the 25 libs: **%d / %d**. The libraries are pure ARM (A32) code." % (nthumb, t.get("T16", 0) + t.get("T32", 0)))
P("- Function starts used: %s (1.7.6). Data skipped (literal pools, switch tables, zero padding): %s bytes." % (
    format(nstarts, ","), format(sum(by[(V, l)]["data_stats"].get("data_bytes", 0) for l in libs), ",")))
nund = sum(len(by[(V, l)]["unknown"]) for l in libs)
nunm = sum(len(by[(V, l)]["unmatched"]) for l in libs)
P("- Words LLVM cannot decode (1.7.6): **%d**. Decoded by LLVM but with no dynarmic decoder entry (1.7.6): **%d**." % (nund, nunm))
stub_used = [(fn, e) for fn, e in dyn.items() if e["status"] == "STUB" and e["n"][V] > 0]
P("- Instructions whose dynarmic handler is a hard stub (Undefined/Interpret): **%d** across %d encodings (1.7.6)." % (
    sum(e["n"][V] for _, e in stub_used), len(stub_used)))
P("")

def cnt_odd(prefix):
    return sum(o["n"][V] for k, o in odd.items() if k.startswith(prefix))
p15 = [k for k in odd if re.match(r"^(mrc|mcr)2? p15", k)]
sha1_n = sum(e["n"] for k, e in unm.items() if k[1].startswith("sha1"))
sha1h_n = sum(c for m_, c in dyn["arm_UDF"]["mn"].items() if m_.startswith("sha1")) if "arm_UDF" in dyn else 0
sha1_libs = sorted({l for k, e in unm.items() if k[1].startswith("sha1") for l in e["libs"]})
wmmx_n = sum(e["n"] for k, e in unm.items() if k[1].startswith(("ldc2", "stc2")))
trap_n = sum(c for (isa, cls, m), c in [((k[0], k[1], k[2]), mn[k][V]) for k in mn] if m in ("trap", "udf"))
neon_libs = {l for k in mn if k[1] == "NEON" for l in mn_libs[k]}
P("## 1b. Findings\n")
P("""1. **The libs are pure ARM-mode code.** 0 Thumb starts, 0 T16, 0 T32 instructions in all 25 libs of all three APKs. dynarmic's Thumb decoders are not exercised by game code (only `bx`/`blx` into HLE stubs matter).
2. **dynarmic decodes everything the libs contain, with one exception: the ARMv8 SHA-1 instructions.** `sha1c/sha1p/sha1m/sha1su0` (%d words, cond=NV, they hit dynarmic's `cccc` ARM patterns and raise `UnpredictableInstruction`) and `sha1h/sha1su1` (%d words, decode to `arm_UDF`, `UndefinedInstruction`) in `sha1_block_data_order` and `_armv8_sha1_probe` of %s. AES (`aese/aesd/aesmc/aesimc`), `sha256h/h2/su0/su1` and `vmull.p64` are decoded and implemented by this dynarmic fork. The only other non-decoded words are 8 NV-coded `ldc2/stc2` in libv's iWMMXt unwinder helpers.
3. **The SHA-1 gap is statically dead code.** These are OpenSSL 1.0.2h CRYPTOGAMS paths selected through `OPENSSL_armcap_P`. In all three builds of libApplication and libSecurity `OPENSSL_cpuid_setup` is a 4-byte `bx lr` stub, and in libApplication 1.7.6 there is no `bl` and no relocated pointer to any `_armv7_neon_probe/_armv7_tick/_armv8_*_probe` function (checked), so `OPENSSL_armcap_P` stays 0 and the plain ARMv4 code paths run (an unlikely non-zero initializer of the variable was not checked). Consequently the OpenSSL NEON/crypto/`mrrc p15 c14` code is never executed and needs no HLE support (no `getauxval`, no SIGILL probing). If we ever want to be safe anyway, make `getauxval`-style queries report no crypto bits.
4. **No privileged TLS or cache maintenance code.** `mrc/mcr p15`: %s (the C++/ObjC TLS access goes through libc `__get_tls`/`pthread_getspecific`, which are HLE). No `wfe/wfi/sev/swp/cps/bkpt/svc` instruction anywhere (zero `svc`: no raw syscalls, every kernel interaction goes through libc imports that we HLE), no Thumb `it` blocks.
5. **Odd but supported:** `ldrex/strex` pairs (%d), `dmb` (%d), `vmrs APSR_nzcv, fpscr` (%d), `msr APSR_nzcvq` (libv `__wrap_memcpy/__wrap_bzero`, handled by `arm_MSR_reg` incl. GE bits), `pld` (2), `nop` hints, `udf`/`trap` (%d, crash paths such as `___forwarding___`, libdispatch assertions), `mrrc p15,#1,..,c14` in OpenSSL `_armv7_tick` (generic-timer counter read; dead, see 3), coprocessor `ldc/stc/ldc2/stc2` on cp1 only in the iWMMXt unwinder helpers of libv (`__gnu_Unwind_Save/Restore_WMMXD/C`; the %d `ldc2/stc2` words are NV-coded and map to dynarmic's `cccc` patterns, i.e. `UnpredictableInstruction`; never executed on a non-iWMMXt CPU).
6. **NEON is small and concentrated**: %d instructions (%d distinct mnemonics) in %d libs (%s). Most is OpenSSL assembly (bit-sliced AES, GHASH, SHA-256/512, `bn_mul_mont`; dead per 3) and the live part is Skia `memset32_neon`/`memset16_neon` (libCoreGraphics) and GSpng (libpng) NEON filter loops (libGraphicsServices); VFP is scalar VFPv3/v4 only (%d instructions, no VFP vector mode, so FPSCR.LEN/STRIDE stay 1 and `vfp_VMOV_imm`'s Undefined guard is never taken).
7. **All 1.7.x builds are the same ISA mix.** The three versions differ by at most %d instructions in total (`libApplication`, `libCoreGraphics` rebuilds).
""" % (sha1_n, sha1h_n, ", ".join(sha1_libs), (", ".join("%s (%d)" % (k, odd[k]["n"][V]) for k in p15) or "none in any lib"),
       cnt_odd("ldrex"), cnt_odd("dmb"), cnt_odd("vmrs APSR_nzcv"), trap_n, wmmx_n,
       sum(n for (i, c, m), n in [((k[0], k[1], k[2]), mn[k][V]) for k in mn] if c == "NEON"),
       sum(1 for k in mn if k[1] == "NEON" and mn[k][V] > 0), len(neon_libs), ", ".join(sorted(neon_libs)),
       sum(n for (i, c, m), n in [((k[0], k[1], k[2]), mn[k][V]) for k in mn] if c == "VFP"),
       max(sum(tot_isa[v].values()) for v in versions) - min(sum(tot_isa[v].values()) for v in versions)))
P("## 2. Method\n")
P("""1. **Starts**: `.dynsym` (bit0 = Thumb), `.ARM.exidx` entries, ObjC2 method IMPs (class and category method lists, parsed from `__objc_classlist` / `__objc_catlist`), `.init_array`/`.fini_array`, and `R_ARM_RELATIVE`/`ABS32` data pointers into executable sections (vtables, function-pointer tables). Source counts per lib are in `opcode-census-libs.csv`.
2. **Mode** (ARM vs Thumb): taken from bit0 when a symbol/pointer source gives it. exidx-only starts (no mode info) use a heuristic: >= 8 of the first 16 words have a condition nibble of 0xE/0xF means ARM, otherwise Thumb.
3. **Regions**: a function is `[start, next_start)` per executable section (`.plt`, `.text`, `text_env`). A synthetic ELF carries `$a/$t/$d` mapping symbols and `llvm-objdump -d --triple=armv8a --mcpu=cortex-a53 --mattr=+neon,+vfp4,+fp16,+hwdiv,+hwdiv-arm,+dsp,+crc,+crypto` decodes it. A superset CPU is used on purpose so nothing is hidden by a feature gate (the libs contain ARMv8 crypto words).
4. **Data skipping** (`$d`), applied in this order:
   - ARM words with the top 16 bits zero (cond=EQ `andeq r0,r0,..` look-alikes: `-O0` switch-table offsets, small constants);
   - **section-wide flow analysis** (ARM): seeds are all region starts, `push {..,lr}` prologues, PIC code addresses (`ldr rX,[pc,#n]` + `add rD,pc,rX`), `adr`+offset tables and unreachable AL islands of >= 4 words; it follows `b/bl` (also across regions), conditional branches and `add{cond} pc,pc,rX,lsl #n` inline `b` tables, and stops at `bx`, `pop {..pc}`, `ldr pc`, `mov/add pc`, runs of >= 3 nops, inline ASCII words and `0xF0/0xFF`-prefixed non-instruction words. Unreachable words that are not a plausible code island are data: non-AL condition nibbles (AES/SHA tables, strings, pools), ASCII words, padding;
   - targets of PC-relative `ldr/ldrb/ldrh/ldrsb/ldrsh/ldrd/vldr` (iterated to a fixpoint), `tbb/tbh` tables (size from the preceding `cmp #imm`), `ldr pc,[pc,rX,lsl #2]` tables, and trailing zero padding.
5. **Self-check**: every direct `b/bl/blx` in the decoded stream is checked against the data map. Result for all 25 libs of 1.7.6: %(btt)s direct branches land in text, %(bid)s of them (%(bpct).3f%%) land on words marked data; the leftovers are dead `b` instructions after `__cxa_throw` in libcxx that branch to literal pools. Hence essentially no real code was classified as data.
6. **Classes**: A32, T16, T32, VFP (scalar VFPv3/v4), NEON (ASIMD), CRYPTO (ARMv8 AES/SHA/PMULL.P64 in AArch32), COPROC (`mcr/mrc/cdp/ldc/stc`), SYSTEM (exclusives, barriers, `svc/udf/bkpt`, hints, `swp`, `msr/mrs`, `pld`...). Encoding bits decide NEON vs VFP. Coprocessor words for cp0-7 that LLVM refuses in v8 mode (iWMMXt save/restore in the unwinder) are classed from their bits.
7. **Dynarmic diff** is by encoding, not by name: each decoded word is matched against dynarmic's own `arm.inc`, `vfp.inc`, `asimd.inc`, `thumb16.inc`, `thumb32.inc` using dynarmic's table order (ARM: VFP, ASIMD, ARM; Thumb: T16, or VFP/ASIMD(converted)/T32). An ARM word with cond=NV (0xF) that only hits a `cccc` pattern is counted as **not decoded**, because dynarmic then raises `Exception::UnpredictableInstruction`. The translator handler body in `translate/impl/*.cpp` is scanned: STUB = body is only `return UndefinedInstruction/InterpretThisInstruction/UnpredictableInstruction()`. PARTIAL = such a call exists on some path (architectural validity guards).
""" % dict(btt=format(btt, ","), bid=bid, bpct=100.0 * bid / max(btt, 1)))
P("## 3. Totals per version\n")
tbl(["class"] + versions, [[c] + [format(tot_cls[v].get(c, 0), ",") for v in versions] for c in classes] +
    [["**total**"] + [format(sum(tot_cls[v].values()), ",") for v in versions]])
P("Per-lib figures for every version are in `opcode-census-libs.csv`; the full mnemonic table is `opcode-census.csv`.\n")
P("### Per lib (1.7.6)\n")
rows = []
for l in libs:
    d = by[(V, l)]
    ds = d["data_stats"]
    rows.append([l, format(d["size"], ","), d["starts"], format(d["insn_total"].get("A", 0), ","), d["insn_total"].get("T16", 0) + d["insn_total"].get("T32", 0),
                 len(d["unknown"]), len(d["unmatched"])])
tbl(["lib", "bytes", "starts", "A32 insns", "Thumb insns", "LLVM-undecodable", "no dynarmic decoder"], rows)

# class breakdowns
def listclass(c, lim=None):
    rows = [(k[2], mn[k][V], len(mn_libs[k])) for k in mn if k[1] == c and mn[k][V] > 0]
    rows.sort(key=lambda r: -r[1])
    return rows[:lim] if lim else rows
P("## 4. Instruction mix (1.7.6)\n")
for c, lim in (("A32", 40), ("VFP", None), ("NEON", None), ("CRYPTO", None), ("COPROC", None), ("SYSTEM", None)):
    rows = listclass(c, lim)
    P("### %s (%d distinct mnemonics%s)\n" % (c, len(listclass(c)), ", top %d shown" % lim if lim else ""))
    if rows:
        tbl(["mnemonic", "count", "libs"], [[m, format(n, ","), nl] for m, n, nl in rows])
    else:
        P("none\n")

P("## 5. Diff against dynarmic A32 decoder\n")
P("### 5.1 Encodings decoded by LLVM but matched by no dynarmic decoder entry (1.7.6)\n")
if unm:
    rows = []
    for k, e in sorted(unm.items(), key=lambda kv: -kv[1]["n"]):
        rows.append([k[0], k[1], e["n"], ", ".join(sorted(e["libs"])), "; ".join("%s:%s" % (l, f_) for (l, f_), _ in e["funcs"].most_common(3))])
    tbl(["isa", "mnemonic", "count", "libs", "top functions"], rows)
else:
    P("none\n")
P("### 5.2 Instructions LLVM cannot decode at all (1.7.6)\n")
if unk:
    tbl(["isa", "count", "libs", "top functions"],
        [[k, e["n"], ", ".join(sorted(e["libs"])), "; ".join("%s:%s" % (l, f_) for (l, f_), _ in e["funcs"].most_common(4))] for k, e in unk.items()])
else:
    P("none\n")
P("### 5.3 Matched, but dynarmic's handler is a stub or can reach Undefined/Interpret\n")
P("STUB handlers that our libs hit (1.7.6): \n")
if stub_used:
    tbl(["matcher", "name", "special call", "count 1.7.4 / 1.7.5 / 1.7.6", "mnemonics (1.7.6)", "libs", "top functions"],
        [[fn, e["name"], e["detail"], " / ".join(str(e["n"][v]) for v in versions),
          ", ".join("%s=%d" % kv for kv in e["mn"].most_common(6)), ", ".join(sorted(e["libs"])),
          "; ".join("%s:%s=%d" % (l, f_, c) for (l, f_), c in e["funcs"].most_common(4))] for fn, e in sorted(stub_used, key=lambda kv: -kv[1]["n"][V])])
else:
    P("none\n")
P("PARTIAL handlers whose body can reach `UndefinedInstruction` or `InterpretThisInstruction` (not just `UnpredictableInstruction`) and that our libs hit (1.7.6). These are architectural validity guards (odd Q-register numbers, reserved size fields, FPSCR.LEN/STRIDE != 1 for VFP), not decoder gaps; listed for completeness:\n")
rows = [[fn, e["name"], e["detail"], " / ".join(str(e["n"][v]) for v in versions)] for fn, e in dyn.items()
        if e["status"] == "PARTIAL" and ("Undefined" in e["detail"] or "Interpret" in e["detail"]) and e["n"][V] > 0]
rows.sort(key=lambda r: -int(r[3].split(" / ")[-1]))
if rows:
    tbl(["matcher", "name", "calls", "count 1.7.4 / 1.7.5 / 1.7.6"], rows)
else:
    P("none\n")
P("Decoder entries that are commented out in dynarmic (known gaps): %s. Nothing in our libs matched any of them unless it appears in 5.1.\n" %
  ", ".join("%s (%s)" % (m.name, m.table) for m in tabs.commented))
P("### 5.4 Dynarmic matchers exercised by our libs\n")
P("%d distinct dynarmic decoder entries are exercised (1.7.6); the complete list with counts and top functions is `opcode-census-dynarmic.csv`.\n" %
  sum(1 for e in dyn.values() if e["n"][V] > 0))

P("## 6. Privileged and odd instructions\n")
P("Full list with top functions: `opcode-census-odd.csv`. `mrc`/`mcr`/`svc`/`udf`/`bkpt`/`swp`/`cps`/`msr`/`mrs`/`vmrs`/`vmsr` are keyed by full text, others by mnemonic.\n")
rows = []
for k in sorted(odd, key=lambda k: -odd[k]["n"][V]):
    o = odd[k]
    if o["n"][V] == 0:
        continue
    rows.append([k, " / ".join(str(o["n"][v]) for v in versions), ", ".join(sorted(o["libs"]))[:70],
                 "; ".join("%s:%s" % (l, f_) for (l, f_), _ in o["funcs"].most_common(3))])
tbl(["instruction", "1.7.4 / 1.7.5 / 1.7.6", "libs", "top functions"], rows)
P("## 7. Caveats\n")
P("""- Coverage is by static decode of symbol-bounded regions; functions reachable only through unnamed pointers with no relocation and no exidx entry would be seen as the tail of their predecessor region, still decoded, only not separately attributed.
- Words whose top 16 bits are zero are treated as data (see Method 4). If any real instruction hides there it would be a conditional-EQ `andeq r0, r0, rX`-class op, which is not generated by compilers.
- LLVM decodes with a superset CPU. A construct LLVM accepts but dynarmic lacks shows up in 5.1; a construct LLVM rejects shows up in 5.2.
- PARTIAL handlers mostly guard UNPREDICTABLE encodings (PC as a register operand). Whether a given use hits that path is data dependent and is not decided statically here.
- The 1.7.4/1.7.5/1.7.6 counts differ only where the library builds differ (`libApplication`, `libCoreGraphics`); other libs are bit-identical apart from build metadata.
""")
open(os.path.join(docs, "opcode-census.md"), "w").write("\n".join(L))
print("\n".join(L[:30]))
