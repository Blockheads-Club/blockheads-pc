/* SPDX-License-Identifier: GPL-3.0-or-later
 * M0 fixture: exercises thunks, ARM/Thumb interworking, NEON, VFP (softfp) and relocations.
 * Own code, built with the NDK for armeabi-v7a. */
#include <arm_neon.h>
#include <stdint.h>
#include <stddef.h>

/* Host-provided imports (resolved to SVC thunks by bhpc-loader). */
extern int puts(const char *s);
extern void bhpc_check(const char *name, int ok);
extern double bhpc_hypot(double a, double b);
extern uint64_t bhpc_mix(uint32_t a, uint32_t b, uint32_t c, uint32_t d, uint32_t e, uint32_t f);

/* Exercises R_ARM_ABS32 / RELATIVE relocations through data tables. */
static const char *const g_names[] = {"alpha", "beta", "gamma"};
static int g_counter = 5;
static int *const g_counter_ptr = &g_counter;
int bhpc_exported_var = 1234;

__attribute__((target("arm"), noinline)) int arm_add3(int a, int b, int c) { return a + b + c; }
__attribute__((target("thumb"), noinline)) int thumb_mul(int a, int b) { return a * b; }

typedef int (*binop3)(int, int, int);
static binop3 const g_fn_table[] = {arm_add3};

/* Thumb calling ARM calling Thumb. */
__attribute__((target("thumb"), noinline)) int thumb_chain(int x) {
  return thumb_mul(arm_add3(x, 1, 2), 2);
}
__attribute__((target("arm"), noinline)) int arm_chain(int x) { return thumb_chain(x) + 1; }

__attribute__((target("arm"), noinline)) int neon_sum(const uint8_t *p, int n) {
  uint32x4_t acc = vdupq_n_u32(0);
  int i = 0;
  for (; i + 16 <= n; i += 16) {
    uint8x16_t v = vld1q_u8(p + i);
    uint16x8_t lo = vpaddlq_u8(v);
    acc = vaddq_u32(acc, vpaddlq_u16(lo));
  }
  uint32_t t[4];
  vst1q_u32(t, acc);
  int s = (int)(t[0] + t[1] + t[2] + t[3]);
  for (; i < n; i++) s += p[i];
  return s;
}

__attribute__((target("arm"), noinline)) float neon_dot(const float *a, const float *b, int n) {
  float32x4_t acc = vdupq_n_f32(0.0f);
  for (int i = 0; i + 4 <= n; i += 4) acc = vmlaq_f32(acc, vld1q_f32(a + i), vld1q_f32(b + i));
  float32x2_t s = vadd_f32(vget_low_f32(acc), vget_high_f32(acc));
  return vget_lane_f32(s, 0) + vget_lane_f32(s, 1);
}

__attribute__((noinline)) double vfp_poly(double x) { return ((3.0 * x + 2.0) * x - 1.0) / 4.0; }
__attribute__((noinline)) double vfp_sqrt(double x) { return __builtin_sqrt(x); }

extern int bhpc_cpp_selftest(void); /* hello.cpp */

int hello_main(void) {
  int fails = 0;
#define CHECK(name, cond) do { int ok_ = (cond) ? 1 : 0; bhpc_check(name, ok_); fails += !ok_; } while (0)
  puts("hello from ARM");
  CHECK("reloc.abs32.string_table", g_names[2][0] == 'g' && g_names[1][1] == 'e');
  CHECK("reloc.relative.data_ptr", *g_counter_ptr == 5);
  CHECK("reloc.fn_table", g_fn_table[0](1, 2, 3) == 6);
  CHECK("interwork.arm", arm_add3(10, 20, 30) == 60);
  CHECK("interwork.thumb", thumb_mul(6, 7) == 42);
  CHECK("interwork.chain", arm_chain(5) == 17);
  uint8_t buf[37];
  int expect = 0;
  for (int i = 0; i < 37; i++) { buf[i] = (uint8_t)(i * 7 + 3); expect += buf[i]; }
  CHECK("neon.sum_u8", neon_sum(buf, 37) == expect);
  float a[8] = {1, 2, 3, 4, 5, 6, 7, 8}, b[8] = {8, 7, 6, 5, 4, 3, 2, 1};
  CHECK("neon.dot_f32", neon_dot(a, b, 8) == 120.0f);
  CHECK("vfp.poly", vfp_poly(2.0) == 3.75);
  CHECK("vfp.sqrt", vfp_sqrt(2.25) == 1.5);
  CHECK("thunk.double_args", bhpc_hypot(3.0, 4.0) == 5.0);
  CHECK("thunk.stack_args_u64", bhpc_mix(1, 2, 3, 4, 5, 6) == 0x0000000600000015ull);
  CHECK("cpp.throw_catch", bhpc_cpp_selftest() == 0);
  return fails;
}
