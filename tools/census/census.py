#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Symbol-bounded ARM/Thumb opcode census of one 32-bit ARM ELF shared object.

usage: census.py LIB.so OUT.json [DYNARMIC_CHECKOUT]

Function starts come from .dynsym (bit0 = Thumb), .ARM.exidx, ObjC2 method
IMPs (class + category method lists), .init_array/.fini_array, and R_ARM_RELATIVE
data pointers into executable sections (vtables, function-pointer tables).
Each region [start, next_start) is decoded by llvm-objdump with a synthetic
ELF carrying $a/$t/$d mapping symbols, so literal pools (found by decoding
PC-relative loads), tbb/tbh tables and trailing padding are never decoded as
instructions.
"""
import bisect
import json
import os
import re
import struct
import subprocess
import sys
import tempfile
from collections import Counter, defaultdict

import numpy as np
from elftools.elf.elffile import ELFFile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import dynarmic_tables as dt  # noqa: E402

OBJDUMP = os.environ.get("LLVM_OBJDUMP", "llvm-objdump")
CONDS = ("eq", "ne", "cs", "hs", "cc", "lo", "mi", "pl", "vs", "vc", "hi", "ls", "ge", "lt", "gt", "le")


class Lib:
    def __init__(self, path):
        self.path = path
        self.data = open(path, "rb").read()
        self.e = ELFFile(open(path, "rb"))
        self.segs = [(p["p_vaddr"], p["p_offset"], p["p_filesz"]) for p in self.e.iter_segments()
                     if p["p_type"] == "PT_LOAD"]
        self.secs = list(self.e.iter_sections())
        self.exec = [(s["sh_addr"], s["sh_size"], s["sh_offset"], s.name) for s in self.secs
                     if s["sh_flags"] & 4 and s["sh_type"] == "SHT_PROGBITS"]

    def off(self, v):
        for va, of, sz in self.segs:
            if va <= v < va + sz:
                return of + v - va

    def r32(self, v):
        o = self.off(v)
        return struct.unpack_from("<I", self.data, o)[0] if o is not None and o + 4 <= len(self.data) else None

    def cstr(self, v):
        o = self.off(v)
        if o is None:
            return None
        return self.data[o:self.data.index(b"\0", o)].decode("latin1")

    def in_exec(self, a):
        for s, n, _, _ in self.exec:
            if s <= a < s + n:
                return True
        return False

    def sec(self, frag):
        for s in self.secs:
            if frag in s.name:
                return s


def collect_starts(lib):
    """returns starts{addr:{'thumb':set(bool),'src':set,'name':str}}"""
    st = {}

    def add(addr, thumb, src, name=None):
        if not lib.in_exec(addr):
            return
        d = st.setdefault(addr, {"thumb": set(), "src": set(), "name": None})
        if thumb is not None:
            d["thumb"].add(thumb)
        d["src"].add(src)
        if name and (not d["name"] or src == "dynsym"):
            d["name"] = name

    dynsym = lib.e.get_section_by_name(".dynsym")
    symv = {}
    for s in dynsym.iter_symbols():
        if s["st_shndx"] != "SHN_UNDEF" and s.name:
            v = s["st_value"]
            symv[s.name] = v
            if s["st_info"]["type"] in ("STT_FUNC", "STT_NOTYPE", "STT_GNU_IFUNC"):
                add(v & ~1, bool(v & 1), "dynsym", s.name)
    # exidx
    ex = lib.sec(".ARM.exidx")
    if ex is not None:
        a, n = ex["sh_addr"], ex["sh_size"]
        for i in range(0, n, 8):
            w = lib.r32(a + i)
            if w & 0x80000000:
                continue
            rel = w - (1 << 31) if w & 0x40000000 else w
            add((a + i + rel) & 0xFFFFFFFF, None, "exidx")
    # init/fini arrays
    for nm in (".init_array", ".fini_array"):
        s = lib.sec(nm)
        if s is not None:
            for i in range(0, s["sh_size"], 4):
                v = lib.r32(s["sh_addr"] + i)
                if v:
                    add(v & ~1, bool(v & 1), "initarr")
    # relocation pointers into text
    rd = lib.sec(".rel.dyn")
    if rd is not None:
        raw = rd.data()
        syms = list(dynsym.iter_symbols())
        for r_off, r_info in struct.iter_unpack("<II", raw):
            t = r_info & 0xFF
            if t == 23:  # RELATIVE
                v = lib.r32(r_off)
            elif t == 2 and (r_info >> 8) < len(syms) and syms[r_info >> 8]["st_shndx"] != "SHN_UNDEF":
                v = (syms[r_info >> 8]["st_value"] + (lib.r32(r_off) or 0)) & 0xFFFFFFFF
            else:
                continue
            if v and lib.in_exec(v & ~1):
                add(v & ~1, bool(v & 1), "reloc")
    # ObjC IMPs
    objc = {}

    def ptrs(frag):
        s = lib.sec(frag)
        if s is None:
            return []
        return [lib.r32(s["sh_addr"] + i) for i in range(0, s["sh_size"], 4)]

    def methods(ml, cls, plus):
        if not ml or lib.off(ml) is None:
            return
        cnt = lib.r32(ml + 4)
        if cnt is None or cnt > 100000:
            return
        for i in range(cnt):
            b = ml + 8 + i * 12
            sel, imp = lib.cstr(lib.r32(b)), lib.r32(b + 8)
            if imp:
                nm = "%s[%s %s]" % ("+" if plus else "-", cls, sel)
                add(imp & ~1, bool(imp & 1), "objc", nm)
                objc[imp & ~1] = nm

    def ro_methods(cp, cname):
        if not cp or cp == 0xFFFFFFFF or lib.off(cp + 16) is None:
            return
        ro = lib.r32(cp + 16) & ~3
        cname = lib.cstr(lib.r32(ro + 16)) if cname is None else cname
        methods(lib.r32(ro + 20), cname, False)
        meta = lib.r32(cp)
        if meta and lib.off(meta + 16) is not None:
            mro = lib.r32(meta + 16) & ~3
            methods(lib.r32(mro + 20), cname, True)

    for cp in ptrs("__objc_classlist"):
        try:
            ro_methods(cp, None)
        except Exception:
            pass
    for cat in ptrs("__objc_catlist"):
        try:
            cn = lib.cstr(lib.r32(cat))
            cls = lib.r32(cat + 4)
            cname = None
            if cls and lib.off(cls + 16) is not None:
                ro = lib.r32(cls + 16) & ~3
                cname = lib.cstr(lib.r32(ro + 16))
            cname = "%s(%s)" % (cname or "?", cn)
            methods(lib.r32(cat + 8), cname, False)
            methods(lib.r32(cat + 12), cname, True)
        except Exception:
            pass
    return st


def decide_mode(lib, st):
    """returns (starts sorted, mode dict addr->'a'|'t', stats)"""
    mode, stats = {}, Counter()
    for a, d in st.items():
        if len(d["thumb"]) == 1:
            mode[a] = "t" if next(iter(d["thumb"])) else "a"
            stats["mode_from_bit0"] += 1
        elif len(d["thumb"]) == 2:
            mode[a] = "t"
            stats["mode_conflict"] += 1
        else:
            # exidx-only: ARM code is dominated by cond=AL (0xE) top nibbles
            o = lib.off(a)
            nxt = min(64, 4 * 16)
            words = struct.unpack_from("<16I", lib.data, o) if o is not None and o + nxt <= len(lib.data) else ()
            e = sum(1 for w in words if (w >> 28) >= 0xE)
            mode[a] = "a" if words and e >= 8 else "t"
            stats["mode_heuristic_" + mode[a]] += 1
    return sorted(st), mode, stats


# ---- literal pool / table discovery -----------------------------------------
def sx(v, bits):
    return v - (1 << bits) if v & (1 << (bits - 1)) else v


def walk(buf, base, regions, data):
    """regions: list of (start,end,mode). data: bool array over buf (offset=addr-base).
    Returns new data marks [(addr,size)] and counters of unbounded tables."""
    new = []
    unb = Counter()

    def lit(target, size):
        o = target - base
        if 0 <= o < len(buf) and not data[o]:
            new.append((target, size))

    for s, e, m in regions:
        pc = s
        lastcmp = (None, 0)  # imm, instr index
        n = 0
        if m == "a":
            while pc + 4 <= e:
                o = pc - base
                if data[o]:
                    pc += 4
                    continue
                w = struct.unpack_from("<I", buf, o)[0]
                n += 1
                c = w >> 28
                if (w & 0x0E1F0000) == 0x041F0000 and c != 0xF:  # ldr/ldrb literal
                    imm = w & 0xFFF
                    t = pc + 8 + (imm if w & (1 << 23) else -imm)
                    lit(t & ~3, 4)
                if (w & 0x0E5F0090) == 0x014F0090 and c != 0xF and (w & 0x60) != 0:  # ldrh/sb/sh/d literal
                    imm = ((w >> 4) & 0xF0) | (w & 0xF)
                    t = pc + 8 + (imm if w & (1 << 23) else -imm)
                    isd = (w & 0x60) == 0x40 and not (w & (1 << 20))
                    lit(t & ~3, 8 if isd else 4)
                if (w & 0x0F3F0E00) == 0x0D1F0A00 and c != 0xF:  # vldr literal
                    imm = (w & 0xFF) * 4
                    t = pc + 8 + (imm if w & (1 << 23) else -imm)
                    lit(t & ~3, 8 if (w & 0x100) else 4)
                if (w & 0x0FFFF000) == 0x079FF000:  # ldr pc,[pc,rX,lsl #2] jump table
                    if lastcmp[0] is not None and n - lastcmp[1] <= 6:
                        lit(pc + 8, 4 * (lastcmp[0] + 1))
                    else:
                        unb["arm_ldrpc_table"] += 1
                if (w & 0x0FF00000) == 0x03500000 and (w >> 16) & 0xF != 0xF:  # cmp rN,#imm
                    if ((w >> 8) & 0xF) == 0:
                        lastcmp = (w & 0xFF, n)
                pc += 4
        else:
            while pc + 2 <= e:
                o = pc - base
                if data[o]:
                    pc += 2
                    continue
                h = struct.unpack_from("<H", buf, o)[0]
                n += 1
                if h < 0xE800:
                    if (h & 0xF800) == 0x4800:
                        lit(((pc + 4) & ~3) + (h & 0xFF) * 4, 4)
                    elif (h & 0xF800) == 0x2800:  # cmp rN,#imm8
                        lastcmp = (h & 0xFF, n)
                    pc += 2
                    continue
                if pc + 4 > e:
                    break
                h2 = struct.unpack_from("<H", buf, o + 2)[0]
                if (h & 0xFF7F) in (0xF85F, 0xF81F, 0xF83F, 0xF91F, 0xF93F):  # ldr* / pld literal
                    imm = h2 & 0xFFF
                    lit(((pc + 4) & ~3) + (imm if h & 0x80 else -imm), 4)
                if (h & 0xFE5F) == 0xE85F:  # ldrd literal
                    lit((((pc + 4) & ~3) + ((h2 & 0xFF) * 4) * (1 if h & 0x80 else -1)), 8)
                if (h & 0xFF3F) == 0xED1F:  # vldr literal
                    lit((((pc + 4) & ~3) + ((h2 & 0xFF) * 4) * (1 if h & 0x80 else -1)), 8 if (h2 & 0x100) else 4)
                if (h & 0xFFF0) == 0xE8D0 and (h2 & 0xFFE0) == 0xF000:  # tbb/tbh
                    hw = bool(h2 & 0x10)
                    if lastcmp[0] is not None and n - lastcmp[1] <= 8:
                        sz = (lastcmp[0] + 1) * (2 if hw else 1)
                        lit(pc + 4, (sz + 1) & ~1)
                    else:
                        unb["thumb_tb_table"] += 1
                if (h & 0xFBF0) == 0xF1B0 and (h2 & 0x8F00) == 0x0F00 and (h2 & 0x7000) == 0:
                    lastcmp = (h2 & 0xFF, n)  # cmp.w rN,#imm8
                pc += 4
    return new, unb



def sx24(w):
    v = w & 0xFFFFFF
    return v - (1 << 24) if v & 0x800000 else v


def junk_runs(buf, base, regions, data):
    """Section-wide ARM flow analysis. Seeds: every ARM region start. Follows direct b/bl (also across regions),
    fallthrough, conditional branches, inline `add pc,pc,rX` branch tables and `adr`+offset tables.
    Unreachable words are classified: words with a non-AL condition nibble are data, AL runs of >= 4 words stay code
    (landing pads, dead code), shorter AL runs next to data are data (random constants), non-AL runs of <= 2 words
    flanked by >= 4 AL words stay code. Returns [(addr,size)] data runs."""
    size = len(buf)
    n = size // 4
    words = np.frombuffer(buf[:n * 4], dtype="<u4")
    seen = np.zeros(n, dtype=bool)
    isdata = data[:n * 4].reshape(n, 4)[:, 0].copy()
    arm = np.zeros(n, dtype=bool)
    stack = []
    for s_, e_, m_ in regions:
        if m_ == "a":
            arm[(s_ - base) // 4:(e_ - base) // 4] = True
            stack.append((s_ - base) // 4)
        else:
            seen[(s_ - base) // 4:(e_ - base + 3) // 4] = True  # thumb code is not analysed here

    bb = words.view(np.uint8).reshape(n, 4)
    stopw = ((bb >= 0x20) & (bb <= 0x7E)).all(axis=1) | ((words >> 24) == 0xFF) | ((words >> 24) == 0xF0)
    # runs of >= 3 nops (e320f000 / mov r0,r0) are alignment padding between functions, not code
    nopw = (words == 0xE320F000) | (words == 0xE1A00000)
    run3 = nopw.copy()
    run3[:n - 2] &= nopw[1:n - 1] & nopw[2:n]
    run3[n - 2:] = False
    padw = run3.copy()
    padw[1:] |= run3[:-1]
    padw[2:] |= run3[:-2]
    stopw = stopw | padw

    def flow(stack):
        while stack:
            i = stack.pop()
            while 0 <= i < n and not seen[i] and not isdata[i] and not stopw[i]:
                seen[i] = True
                w = int(words[i])
                c = w >> 28
                if ((w >> 25) & 7) == 5 and c != 0xF:  # B / BL
                    t = i + 2 + sx24(w)
                    if 0 <= t < n:
                        stack.append(t)
                    if not (w & 0x01000000) and c == 0xE:
                        break
                elif (w & 0x0FFFFFF0) == 0x012FFF10 and c == 0xE:  # bx
                    break
                elif c == 0xE and (w & 0x0E108000) == 0x08108000:  # ldm/pop {..pc}
                    break
                elif c == 0xE and (w & 0x0C100000) == 0x04100000 and ((w >> 12) & 0xF) == 15:  # ldr pc
                    break
                elif c != 0xF and (w & 0x0C000000) == 0 and ((w >> 12) & 0xF) == 15 and ((w >> 21) & 0xF) == 4 and \
                        ((w >> 16) & 0xF) == 15 and (w & 0x0E000090) != 0x00000090:
                    # add{cond} pc,pc,rX,lsl #n : inline table of `b` instructions follows
                    j = i + 1
                    while j < n and (int(words[j]) == 0xE320F000 or (int(words[j]) >> 24) == 0xEA):
                        if (int(words[j]) >> 24) == 0xEA:
                            stack.append(j)
                        j += 1
                elif c == 0xE and (w & 0x0C000000) == 0 and ((w >> 12) & 0xF) == 15 and \
                        ((w >> 21) & 0xF) not in (8, 9, 10, 11) and (w & 0x0E000090) != 0x00000090:  # dp op writing pc
                    break
                i += 1
    # function prologues `push {..., lr}` also seed the flow (static functions reached only via pointers)
    stack += [int(i) for i in np.nonzero(((words & 0xFFFF4000) == 0xE92D4000) & arm & ~isdata)[0]]
    # PIC code addresses: `ldr rX,[pc,#imm]` (pool word W) then `add rD,pc,rX` => target = pc_of_add + 8 + W
    for i in np.nonzero(((words & 0xFF7F0000) == 0xE51F0000) | ((words & 0xFF7F0000) == 0xE59F0000))[0]:
        i = int(i)
        w = int(words[i])
        imm = w & 0xFFF
        p = i + 2 + ((imm if w & 0x00800000 else -imm) // 4)
        if not (0 <= p < n):
            continue
        rx = (w >> 12) & 0xF
        for k in range(1, 5):
            if i + k >= n:
                break
            w2 = int(words[i + k])
            if (w2 & 0xFFFF0FF0) == 0xE08F0000 and (w2 & 0xF) == rx:  # add rD, pc, rX
                t = i + k + 2 + (int(words[p]) >> 2)
                if int(words[p]) & 3 == 0 and 0 <= t < n:
                    stack.append(t)
                break
            if ((w2 >> 12) & 0xF) == rx:  # rX overwritten
                break
    flow(stack)
    # adr + offset tables: add rX,pc,#imm followed by hi16==0 words => targets = base + word
    extra = []
    for i in np.nonzero((words & 0x0FEF0000) == 0x028F0000)[0]:
        w = int(words[i])
        if (w >> 28) != 0xE or not seen[i]:
            continue
        rot, i8 = (w >> 8) & 0xF, w & 0xFF
        imm = ((i8 >> (2 * rot)) | (i8 << (32 - 2 * rot))) & 0xFFFFFFFF if rot else i8
        bi = int(i) + 2 + imm // 4
        j = bi
        while 0 <= j < n and isdata[j] and (int(words[j]) >> 16) == 0:
            t = bi + (int(words[j]) >> 2)
            if 0 <= t < n and not seen[t]:
                extra.append(t)
            j += 1
    flow(extra)
    # Unreachable code islands (>= 4 consecutive AL words that are not strings) act as extra seeds, so dead code,
    # EH landing pads and callbacks reached only through computed pointers pull in their branch targets too.
    al0 = (words >> 28) == 0xE
    for _ in range(4):
        cand = arm & ~seen & ~isdata & al0 & ~stopw
        run4 = cand.copy()
        for k in range(1, 4):
            run4[:n - k] &= cand[k:]
            run4[n - k:] = False
        starts_ = np.nonzero(run4)[0]
        if not len(starts_):
            break
        before = int(seen.sum())
        flow([int(i) for i in starts_])
        if int(seen.sum()) == before:
            break
    out = []
    unreach = arm & ~seen & ~isdata
    # 4 printable-ASCII bytes: inline string, never an instruction compilers emit
    b = words.view(np.uint8).reshape(n, 4)
    ascii_w = unreach & ((b >= 0x20) & (b <= 0x7E)).all(axis=1)
    for i in np.nonzero(ascii_w | (padw & unreach))[0]:
        out.append((int(i), int(i) + 1))
    unreach &= ~(ascii_w | padw)
    al = (words >> 28) == 0xE
    cond = words >> 28
    plausible = (cond >= 1) & (cond <= 0xD)  # conditional (not EQ/NV) words that may be real predicated code
    i = 0
    while i < n:
        if not unreach[i]:
            i += 1
            continue
        j = i
        while j < n and unreach[j]:
            j += 1
        # tokenise [i,j) into AL / non-AL runs
        toks = []
        k = i
        while k < j:
            q = k
            while q < j and al[q] == al[k]:
                q += 1
            toks.append([bool(al[k]), k, q])
            k = q
        for t, (isal, a_, b_) in enumerate(toks):
            prev_al = toks[t - 1] if t else None
            next_al = toks[t + 1] if t + 1 < len(toks) else None
            if isal:
                if b_ - a_ >= 4:
                    continue
                if (prev_al is None) and (next_al is None):
                    continue
                out.append((a_, b_))
            else:
                if b_ - a_ <= 2 and prev_al and next_al and prev_al[2] - prev_al[1] >= 4 and next_al[2] - next_al[1] >= 4 \
                        and plausible[a_:b_].all():
                    continue
                out.append((a_, b_))
        i = j
    return [(base + 4 * a_, 4 * (b_ - a_)) for a_, b_ in out]


def build_data(lib, regions_by_sec, sections):
    """Iterate literal discovery to fixpoint. Returns per-section bool arrays, stats."""
    datas, stat = {}, Counter()
    for name, (base, size, off) in sections.items():
        buf = lib.data[off:off + size]
        data = np.zeros(size, dtype=bool)
        # ARM words whose top 16 bits are zero are cond=EQ "andeq r0,r0,.." look-alikes: compilers never
        # emit them, they are switch-table offsets / small constants / padding.
        for s_, e_, m_ in regions_by_sec[name]:
            if m_ == "a":
                w = np.frombuffer(buf[s_ - base:s_ - base + ((e_ - s_) & ~3)], dtype="<u4")
                z = np.repeat((w >> 16) == 0, 4)
                data[s_ - base:s_ - base + len(z)] |= z
                stat["zero_hi16_words"] += int((w >> 16 == 0).sum())
        for a_, sz_ in junk_runs(buf, base, regions_by_sec[name], data):
            data[a_ - base:a_ - base + sz_] = True
            stat["junk_words"] += sz_ // 4
        for it in range(4):
            new, unb = walk(buf, base, regions_by_sec[name], data)
            if it == 0:
                stat.update(unb)
            if not new:
                break
            for a, sz in new:
                o = a - base
                data[o:o + sz] = True
        # trailing zero padding in each region
        for s, e, m in regions_by_sec[name]:
            unit = 4 if m == "a" else 2
            p = e
            while p - unit >= s and not any(buf[p - unit - base:p - base]):
                p -= unit
            if p < e:
                data[p - base:e - base] = True
                stat["pad_bytes"] += e - p
        stat["data_bytes"] += int(data.sum())
        datas[name] = data
    return datas, stat


def synth_elf(path, base, buf, regions, data, name=".text"):
    """Minimal ET_EXEC ELF32-ARM with one exec section and $a/$t/$d mapping symbols."""
    syms = []  # (name, value)
    size = len(buf)
    for s, e, m in regions:
        cur = None
        p = s
        while p < e:
            isd = bool(data[p - base])
            q = p
            while q < e and bool(data[q - base]) == isd:
                q += 1
            kind = "$d" if isd else ("$a" if m == "a" else "$t")
            if kind != cur:
                syms.append((kind, p))
                cur = kind
            p = q
    strtab = b"\0"
    ents = [struct.pack("<IIIBBH", 0, 0, 0, 0, 0, 0)]
    for nm, v in syms:
        ents.append(struct.pack("<IIIBBH", len(strtab), v, 0, 0, 1, 1))  # NOTYPE LOCAL, shndx=1
        strtab += nm.encode() + b"\0"
    symtab = b"".join(ents)
    shstr = b"\0.text\0.symtab\0.strtab\0.shstrtab\0"
    off_text = 52
    off_sym = (off_text + size + 3) & ~3
    off_str = off_sym + len(symtab)
    off_shs = off_str + len(strtab)
    off_sh = (off_shs + len(shstr) + 3) & ~3

    def sh(name, typ, flags, addr, off, sz, link=0, info=0, al=1, es=0):
        return struct.pack("<10I", name, typ, flags, addr, off, sz, link, info, al, es)
    shdrs = (sh(0, 0, 0, 0, 0, 0) +
             sh(1, 1, 6, base, off_text, size, al=4) +
             sh(7, 2, 0, 0, off_sym, len(symtab), link=3, info=1, al=4, es=16) +
             sh(15, 3, 0, 0, off_str, len(strtab)) +
             sh(23, 3, 0, 0, off_shs, len(shstr)))
    ehdr = b"\x7fELF\x01\x01\x01\x00" + b"\0" * 8 + struct.pack("<HHIIIIIHHHHHH", 2, 40, 1, 0, 0, off_sh,
                                                                  0x5000000, 52, 0, 0, 40, 5, 4)
    with open(path, "wb") as f:
        f.write(ehdr)
        f.write(buf)
        f.write(b"\0" * (off_sym - off_text - size))
        f.write(symtab + strtab + shstr)
        f.write(b"\0" * (off_sh - off_shs - len(shstr)))
        f.write(shdrs)


LINE = re.compile(r"^\s*([0-9a-f]+):\s*\t(\S+)(?:\t(.*))?$")


def base_mnem(m, known):
    if "." in m:
        h, _, t = m.partition(".")
        return base_mnem(h, known) + "." + t
    if m in known and known[m] > 0:
        return m
    for c in CONDS:
        if m.endswith(c) and len(m) > 2 and m[:-2] in known:
            return m[:-2]
    return m



def coproc_fallback(isa, enc):
    """Name an ARM coprocessor instruction (cp0-7) from its bits, or None."""
    if isa != "A":
        return None
    nv = (enc >> 28) == 0xF
    if nv and (enc >> 24) not in (0xFC, 0xFD, 0xFE):
        return None
    cp = (enc >> 8) & 0xF
    if cp in (10, 11) or cp > 7:
        return None
    sfx = "2" if nv else ""
    t = (enc >> 25) & 7
    if t == 6:
        if ((enc >> 21) & 0x7F) == 0x62:
            return ("mrrc" if (enc >> 20) & 1 else "mcrr") + sfx
        return ("ldc" if (enc >> 20) & 1 else "stc") + sfx + ("l" if (enc >> 22) & 1 else "")
    if (enc >> 24) & 0xF == 0xE:
        if (enc >> 4) & 1:
            return ("mrc" if (enc >> 20) & 1 else "mcr") + sfx
        return "cdp" + sfx
    return None


def classify(isa, mn, enc):
    """-> census class"""
    b = mn.split(".")[0]
    if re.match(r"(aes|sha1|sha256)[a-z0-9]*\.\d+", mn) or mn.startswith("vmull.p64"):
        return "CRYPTO"
    if isa == "A":
        top = enc >> 28
        if (enc >> 25) == 0b1111001 or (enc & 0xFF100000) == 0xF4000000 or (enc & 0xFE000000) == 0xF2000000:
            return "NEON"
        if b.startswith("v"):
            if re.search(r"\b[qd]\d+\[", mn) or b in ("vdup",):
                return "NEON"
            return "VFP" if b not in ("vdup",) else "NEON"
    else:
        if b.startswith("v"):
            if isa == "T32" and ((enc & 0xEF000000) == 0xEF000000 or (enc & 0xFF000000) == 0xF9000000):
                return "NEON"
            return "VFP"
    return None


SYSTEM = {"ldrex", "ldrexb", "ldrexh", "ldrexd", "strex", "strexb", "strexh", "strexd", "clrex", "dmb", "dsb", "isb",
          "svc", "udf", "bkpt", "wfe", "wfi", "sev", "sevl", "yield", "swp", "swpb", "cps", "msr", "mrs", "pld", "pldw",
          "pli", "hint", "dbg", "smc", "hvc", "rfe", "srs", "eret", "setend", "nop", "csdb", "ssbb", "pssbb",
          "lda", "ldab", "ldah", "ldaex", "stl", "stlb", "stlh", "stlex", "ldaexb", "ldaexh", "ldaexd", "stlexb", "stlexh", "stlexd"}
COPROC = {"mcr", "mrc", "mcr2", "mrc2", "mcrr", "mrrc", "mcrr2", "mrrc2", "cdp", "cdp2", "ldc", "ldc2", "stc", "stc2", "ldcl", "stcl"}
ODD_RE = re.compile(r"^(mrc2?|mcr2?|mcrr2?|mrrc2?|cdp2?|ldc2?l?|stc2?l?|swpb?|wfe|wfi|sev|sevl|yield|svc|udf|bkpt|cps[a-z]*|"
                    r"msr|mrs|clrex|dmb|dsb|isb|ldrex[bhd]?|strex[bhd]?|pld[w]?|pli|setend|smc|hvc|rfe[a-z]*|srs[a-z]*|eret|vmrs|vmsr|dbg|nop)$")


def run(libpath, outpath, dynroot):
    lib = Lib(libpath)
    st = collect_starts(lib)
    starts, mode, mstats = decide_mode(lib, st)
    names = {a: d["name"] for a, d in st.items() if d["name"]}
    srcs_cnt = Counter()
    for d in st.values():
        for s in d["src"]:
            srcs_cnt[s] += 1
    # regions per exec section
    sections, regions_by_sec = {}, {}
    for sa, sn, so, nm in lib.exec:
        sections[nm] = (sa, sn, so)
        ss = [a for a in starts if sa <= a < sa + sn]
        regs = []
        if not ss or ss[0] != sa:
            ss = [sa] + ss
            mode.setdefault(sa, "a")
        for i, a in enumerate(ss):
            e = ss[i + 1] if i + 1 < len(ss) else sa + sn
            regs.append((a, e, mode[a]))
        regions_by_sec[nm] = regs
    datas, dstat = build_data(lib, regions_by_sec, sections)

    mnem = Counter()          # (isa, cls, mnemonic_full) -> n
    funcs_unknown = Counter()
    insn_total = Counter()
    encs = {"A": [], "T16": [], "T32": []}
    info = {"A": [], "T16": [], "T32": []}  # parallel: (addr, func_start, mnemonic)
    odd = []                  # (addr, func, isa, text)
    unknown = []              # (addr, func, isa, bytes)
    tmp = tempfile.mkdtemp(prefix="census_")
    known_cnt = Counter()
    rows = []  # (sec, addr, size, isa, mn, ops, enc)
    for nm, (base, size, off) in sections.items():
        buf = lib.data[off:off + size]
        regs = regions_by_sec[nm]
        data = datas[nm]
        p = os.path.join(tmp, "s.elf")
        synth_elf(p, base, buf, regs, data, nm)
        r = subprocess.run([OBJDUMP, "-d", "--no-show-raw-insn", "--triple=armv8a", "--mcpu=cortex-a53",
                            "--mattr=+neon,+vfp4,+fp16,+hwdiv,+hwdiv-arm,+dsp,+crc,+crypto", p],
                           capture_output=True, text=True, errors="replace")
        lines = []
        for ln in r.stdout.splitlines():
            m = LINE.match(ln)
            if m:
                lines.append((int(m.group(1), 16), m.group(2).strip(), (m.group(3) or "").strip()))
        reg_starts = [x[0] for x in regs]
        # per-address thumb mode
        for i, (a, mn, ops) in enumerate(lines):
            if mn.startswith("."):
                continue
            ri = bisect.bisect_right(reg_starts, a) - 1
            m_ = regs[ri][2]
            o = a - base
            if m_ == "a":
                isa, sz = "A", 4
                enc = struct.unpack_from("<I", buf, o)[0]
            else:
                h = struct.unpack_from("<H", buf, o)[0]
                if h < 0xE800:
                    isa, sz, enc = "T16", 2, h
                else:
                    isa, sz = "T32", 4
                    enc = (h << 16) | struct.unpack_from("<H", buf, o + 2)[0]
            known_cnt[mn] += 0 if mn == "<unknown>" else 1
            rows.append((a, regs[ri][0], isa, mn, ops, enc))
    # base mnemonics (needs known set)
    known = {k: v for k, v in known_cnt.items()}
    flagged = []
    stat_check, stat_check_samples = Counter(), []
    name_of_early = lambda fs: names.get(fs, 'sub_%x' % fs)
    for a, fs, isa, mn, ops, enc in rows:
        insn_total[isa] += 1
        fb = coproc_fallback(isa, enc) if (mn == "<unknown>" or mn.startswith("<")) else None
        if fb:
            # LLVM (v8 mode) refuses coprocessors p0-p7 (iWMMXt/XScale); decode the class from the bits instead
            mnem[(isa, "COPROC", fb)] += 1
            encs[isa].append(enc)
            info[isa].append((a, fs, fb))
            odd.append((a, fs, isa, fb + " (cp%d, LLVM-v8-undecodable)" % ((enc >> 8) & 0xF), "%08x" % enc))
            continue
        if mn == "<unknown>" or mn.startswith("<"):
            unknown.append((a, fs, isa, "%08x" % enc))
            mnem[(isa, "UNDECODED", "<undecoded>")] += 1
            continue
        bm = base_mnem(mn, known)
        b0 = bm.split(".")[0]
        cls = classify(isa, bm + " " + ops, enc)
        if cls is None:
            if b0 in COPROC:
                cls = "COPROC"
            elif b0 in SYSTEM:
                cls = "SYSTEM"
            else:
                cls = {"A": "A32", "T16": "T16", "T32": "T32"}[isa]
        if bm.split(".")[0] in ("b", "bl", "blx") and ops.startswith("0x"):
            tgt = int(ops.split()[0], 16)
            for nm_, (b_, s_, o_) in sections.items():
                if b_ <= tgt < b_ + s_:
                    stat_check["branches_to_text"] += 1
                    if datas[nm_][tgt - b_]:
                        stat_check["branch_into_data"] += 1
                        if len(stat_check_samples) < 20:
                            stat_check_samples.append([hex(a), hex(tgt), name_of_early(fs)])
        mnem[(isa, cls, bm)] += 1
        encs[isa].append(enc)
        info[isa].append((a, fs, bm))
        if ODD_RE.match(b0) or (b0 in ("vmrs", "vmsr")):
            odd.append((a, fs, isa, "%s %s" % (mn, ops), "%08x" % enc))
    # dynarmic matching
    tabs = dt.Tables(dynroot)
    handlers = dt.analyse_handlers(dynroot)
    dyn_counts = defaultdict(lambda: [0, Counter(), {}, Counter()])  # fn -> [n, funcs, example]
    unmatched = []
    crypto = []
    for isa, fn_match in (("A", tabs.match_arm), ("T16", tabs.match_t16), ("T32", tabs.match_t32)):
        arr = np.array(encs[isa], dtype=np.uint32)
        if not len(arr):
            continue
        res, pool = fn_match(arr)
        for k, ((a, fs, bm), r_, e_) in enumerate(zip(info[isa], res, arr)):
            if re.match(r"(aes|sha1|sha256)[a-z0-9]*\.\d+", bm) or bm.startswith("vmull.p64"):
                crypto.append((bm, "%08x" % e_, pool[r_].fn if r_ >= 0 else None, fs, a))
            nv = isa == "A" and r_ >= 0 and (int(e_) >> 28) == 0xF and (pool[r_].mask >> 28) != 0xF
            if r_ < 0:
                unmatched.append((isa, a, fs, bm, "%08x" % e_))
            elif nv:
                # cond=NV word that only hits a `cccc` pattern: dynarmic raises Exception::UnpredictableInstruction
                unmatched.append((isa, a, fs, bm + " [cond=NV -> " + pool[r_].fn + "]", "%08x" % e_))
            else:
                mm = pool[r_]
                d = dyn_counts[mm.fn]
                d[0] += 1
                st_, det, _ = handlers.get(mm.fn, ("OK", "", ""))
                if st_ != "OK":
                    d[1][fs] += 1
                    d[3][bm] += 1
                    d[2].setdefault("ex", "%08x %s" % (e_, bm))
                    d[2]["status"], d[2]["detail"], d[2]["name"] = st_, det, mm.name
    name_of = lambda fs: names.get(fs, "sub_%x" % fs)
    out = {
        "lib": os.path.basename(libpath),
        "size": os.path.getsize(libpath),
        "starts": len(starts),
        "thumb_starts": sum(1 for a in starts if mode.get(a) == "t"),
        "start_sources": dict(srcs_cnt),
        "mode_stats": dict(mstats),
        "data_stats": dict(dstat),
        "branch_check": dict(stat_check),
        "branch_check_samples": stat_check_samples,
        "insn_total": dict(insn_total),
        "exec_sections": [[nm, sa, sn] for sa, sn, so, nm in lib.exec],
        "mnem": [[isa, cls, m, n] for (isa, cls, m), n in mnem.items()],
        "odd": [[hex(a), name_of(fs), isa, t, e] for a, fs, isa, t, e in odd],
        "unknown": [[hex(a), name_of(fs), isa, e] for a, fs, isa, e in unknown],
        "unmatched": [[isa, hex(a), name_of(fs), bm, e] for isa, a, fs, bm, e in unmatched],
        "dyn": {fn: {"n": v[0], "status": v[2].get("status", "OK"), "detail": v[2].get("detail", ""),
                     "name": v[2].get("name", ""), "example": v[2].get("ex", ""), "mnems": v[3].most_common(30),
                     "funcs": [[name_of(f), c] for f, c in v[1].most_common(200)]}
                for fn, v in dyn_counts.items()},
        "crypto": [[bm, e, m, name_of(fs), hex(a)] for bm, e, m, fs, a in crypto],
        "dyn_all": {fn: v[0] for fn, v in dyn_counts.items()},
    }
    json.dump(out, open(outpath, "w"))
    subprocess.run(["rm", "-rf", tmp])
    return out


if __name__ == "__main__":
    root = sys.argv[3] if len(sys.argv) > 3 else os.environ.get("DYNARMIC_SRC", "")
    o = run(sys.argv[1], sys.argv[2], root)
    print(o["lib"], o["insn_total"], "starts", o["starts"], len(o["unknown"]), "undecoded", len(o["unmatched"]), "unmatched")
