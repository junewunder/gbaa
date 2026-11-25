/* c-resume's restores (resume.s): each loads sp and pc from a block a
 * function of its own filled, not setjmp's buffer. */
#ifndef RESUME_H
#define RESUME_H
/* search(0, limit) under a guard: what it returns, or what a bail gives. */
u32 guard_call(u32 limit);
/* Back into guard_call, which returns v. */
void bail(u32 v);
/* The sum of step(i) for i below n. */
u32 run_list(u32 n);
/* The step under way gives v, and the loop goes on with the next. */
void skip(u32 v);
#endif
