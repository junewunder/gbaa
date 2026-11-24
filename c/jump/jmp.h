/* c-jump's setjmp and longjmp (setjmp.s): r4-r12, sp and lr. */
#ifndef JMP_H
#define JMP_H
typedef u32 jmp_buf[11];
int setjmp(jmp_buf env);
void longjmp(jmp_buf env, int val);
#endif
