# M0 opcode census

Symbol-bounded ARM/Thumb opcode census of the user's `lib/armeabi-v7a/*.so` and a diff against a dynarmic A32 decoder.
Results: `docs/m0/opcode-census.md` (+ CSVs). Nothing from the APKs is stored in the repo.

```
# extract lib/armeabi-v7a/*.so from each APK into its own directory first
git clone --depth 1 https://github.com/azahar-emu/dynarmic /tmp/dynarmic-src
tools/census/run_all.sh /tmp/dynarmic-src /tmp/census-out DIR_174 DIR_175 DIR_176
python3 tools/census/report.py /tmp/census-out docs/m0 /tmp/dynarmic-src "$(git -C /tmp/dynarmic-src rev-parse HEAD)"
```

Needs `llvm-objdump` (LLVM 17+), `python3` with `numpy` and `pyelftools`. `census.py LIB.so OUT.json DYNARMIC_SRC` runs one library.

Data hygiene (G1 finding F4): the committed CSVs and `opcode-census.md` carry addresses, mnemonics and function names
only, never raw instruction words (`encoding_hex`, `example_encoding` and "sample encodings" were dropped). The
analysis scripts keep the words in memory while they run; they are not written to the repository.

These Python tools are outside the Rust coverage gate (`scripts/coverage.sh` measures Rust code only).
