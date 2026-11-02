// RuPix host-side C SEH isolation layer (Windows / MSVC only).
//
// Why this exists: the host loads third-party decoder DLLs in-process and calls their
// exported `iv_*` functions directly. Rust's `catch_unwind` only catches Rust panics; a
// line like `null.write(1)` inside a decoder triggers a real SEH access violation, which
// does not produce an unwind and would take the whole process down.
//
// On x86_64-pc-windows-msvc there is no stable Rust SEH syntax, so the standard trick is
// to wrap the call in C `__try / __except`. This file does exactly that: it catches only
// the "hard fault" exception codes and lets everything else (Rust's own panic, which uses
// a different SEH code) pass through to the host's `catch_unwind`.
//
// We deliberately include NO header: `__try/__except` is a compiler construct and
// `_exception_code()` is an SEH intrinsic (normally declared in <excpt.h>). `GetExceptionCode`
// is a *macro* over that intrinsic, so without <windows.h> it is an unresolved symbol -- we
// call `_exception_code()` directly instead. The `EXCEPTION_*` codes are just constants.
// Avoiding <windows.h> means we do not depend on the Windows SDK include path being
// discoverable from the build script, which is not always the case here.
//
// On a caught fault the shim returns a *class*, not one blanket code: `SEH_DECODER_FAULT`
// for damage the decoder did to itself (goes to the three-strikes penalty) and
// `SEH_MEDIA_FAULT` for `EXCEPTION_IN_PAGE_ERROR` — the medium disappeared under us
// (network share dropped, USB pulled, file truncated mid-read). Charging the second one to
// the decoder both quarantines a healthy decoder and makes the log claim a fault that never
// happened (audit V8). This is still "damage control", not a sandbox: memory the decoder
// scribbled on may still be corrupted, but at least the window does not vanish. True
// isolation is route B (a separate decode process).

// Hard-fault exception codes (identical to the macros in <windows.h>).
#define EXCEPTION_ACCESS_VIOLATION    0xC0000005
#define EXCEPTION_IN_PAGE_ERROR       0xC0000006
#define EXCEPTION_ILLEGAL_INSTRUCTION 0xC000001D
#define EXCEPTION_PRIV_INSTRUCTION    0xC0000096
#define STATUS_STACK_OVERFLOW         0xC00000FD

// sv_seh_call returns a *class*, not one blanket code. 0 comes from the thunk itself
// (0 = normal, -1 = the thunk's own "caller broke the contract"); the two negatives below
// are the fault classes: only decoder damage belongs in the three-strikes penalty, a lost
// medium does not (audit V8).
#define SEH_DECODER_FAULT (-1)
#define SEH_MEDIA_FAULT  (-2)

// After catching a stack overflow the guard page must be re-armed, otherwise the *next*
// deep recursion no longer reports STATUS_STACK_OVERFLOW but simply scribbles past the end
// of the stack -- worse than not catching it at all. Provided by the CRT (normally declared
// in <stdlib.h>; this file deliberately includes no headers, so declare it here. Under the
// static CRT (/MT) it is an ordinary function).
extern int _resetstkoflw(void);

// Codes worth swallowing -> return that class; everything else (Rust's own panic uses a
// different SEH code) -> 0, keep searching.
static int sv_swallow_class(unsigned long code) {
    if (code == EXCEPTION_IN_PAGE_ERROR) {
        return 2; // media fault: the volume / share holding the file stopped serving pages
    }
    if (code == EXCEPTION_ACCESS_VIOLATION || code == EXCEPTION_ILLEGAL_INSTRUCTION ||
        code == EXCEPTION_PRIV_INSTRUCTION || code == STATUS_STACK_OVERFLOW) {
        return 1; // hard fault caused by the decoder itself
    }
    return 0;
}

// Filter return values (identical to the macros in <windows.h>).
#define EXCEPTION_EXECUTE_HANDLER 1
#define EXCEPTION_CONTINUE_SEARCH 0

// SEH intrinsic normally declared in <excpt.h>; declared here to stay header-free.
unsigned long __cdecl _exception_code(void);

typedef int (*sv_thunk_t)(void* ctx);

__declspec(dllexport)
int sv_seh_call(sv_thunk_t thunk, void* ctx) {
    int rc;
    __try {
        rc = thunk(ctx);
    } __except (
        // Swallow only the hard faults: access violation / in-page error / illegal or
        // privileged instruction / **stack overflow** (new in this round -- audit V7: it used
        // to fall through to CONTINUE_SEARCH, so a decoder that recursed too deep aborted the
        // process, and since it was never charged as a decoder fault it also never got
        // blacklisted and crashed again on the next open). Rust's own panic uses a different
        // SEH code and is left to CONTINUE_SEARCH so it unwinds into catch_unwind instead.
        // `_exception_code()` is called directly here (no <windows.h>, so GetExceptionCode
        // -- a macro -- is unavailable).
        sv_swallow_class(_exception_code()) != 0
            ? EXCEPTION_EXECUTE_HANDLER
            : EXCEPTION_CONTINUE_SEARCH) {
        // Reaching here, every frame of the faulting call has already been unwound -- which
        // is exactly why a stack overflow can be survived in `__except` but not in `__finally`:
        // the handler runs on stack that has been given back.
        int cls = sv_swallow_class(_exception_code());
        if (_exception_code() == STATUS_STACK_OVERFLOW) {
            (void)_resetstkoflw(); // re-arm the guard page or the next overflow just tramples memory
        }
        rc = (cls == 2) ? SEH_MEDIA_FAULT : SEH_DECODER_FAULT;
    }
    return rc;
}
