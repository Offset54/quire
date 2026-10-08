/*
 * Guarded copies out of memory-mapped files (MASTER_PROMPT 5.5; ADR-0007).
 *
 * An I/O error behind a mapped view (a network drop, a removed disk, a file
 * truncated by another process) is reported as a hardware exception:
 * EXCEPTION_IN_PAGE_ERROR on Windows, SIGBUS on Unix. Rust can't catch
 * either, so every read of a mapped view goes through quire_guarded_copy,
 * which turns the fault into a return code.
 *
 * - Windows: structured exception handling around memcpy. Only
 *   EXCEPTION_IN_PAGE_ERROR is handled; anything else (an access violation
 *   is a real bug) keeps propagating.
 * - Unix: a SIGBUS handler, installed once, jumps back out of a guarded copy
 *   on the faulting thread. A SIGBUS outside a guarded copy is passed to the
 *   previous handler (Rust's stack-overflow handler, or the default action).
 */

#include <stddef.h>
#include <string.h>

#ifdef _WIN32
#include <windows.h>

static int quire_filter(DWORD code) {
  return code == EXCEPTION_IN_PAGE_ERROR ? EXCEPTION_EXECUTE_HANDLER : EXCEPTION_CONTINUE_SEARCH;
}

int quire_guarded_copy(void *dst, const void *src, size_t len) {
  __try {
    memcpy(dst, src, len);
    return 0;
  } __except (quire_filter(GetExceptionCode())) {
    return 1;
  }
}

/* Raises EXCEPTION_IN_PAGE_ERROR inside the guard, for tests: a real one
 * needs a failing device. Returns 1 if the guard caught it. */
int quire_guard_selftest(void) {
  __try {
    RaiseException(EXCEPTION_IN_PAGE_ERROR, 0, 0, NULL);
    return 0;
  } __except (quire_filter(GetExceptionCode())) {
    return 1;
  }
}

#else
#include <pthread.h>
#include <setjmp.h>
#include <signal.h>

static _Thread_local sigjmp_buf *volatile quire_active = NULL;
static struct sigaction quire_previous;
static pthread_once_t quire_once = PTHREAD_ONCE_INIT;

static void quire_handler(int sig, siginfo_t *info, void *context) {
  sigjmp_buf *env = quire_active;
  if (env != NULL) {
    quire_active = NULL;
    siglongjmp(*env, 1);
  }
  /* Not ours: hand over to whoever was installed before us. */
  if (quire_previous.sa_flags & SA_SIGINFO) {
    if (quire_previous.sa_sigaction != NULL) {
      quire_previous.sa_sigaction(sig, info, context);
      return;
    }
  } else if (quire_previous.sa_handler != SIG_DFL && quire_previous.sa_handler != SIG_IGN) {
    quire_previous.sa_handler(sig);
    return;
  }
  /* Default action: returning re-executes the faulting access, which now
   * kills the process as it would have without us. */
  signal(sig, SIG_DFL);
}

static void quire_install(void) {
  struct sigaction action;
  memset(&action, 0, sizeof action);
  action.sa_sigaction = quire_handler;
  action.sa_flags = SA_SIGINFO | SA_ONSTACK | SA_NODEFER;
  sigemptyset(&action.sa_mask);
  sigaction(SIGBUS, &action, &quire_previous);
}

int quire_guarded_copy(void *dst, const void *src, size_t len) {
  sigjmp_buf env;
  pthread_once(&quire_once, quire_install);
  if (sigsetjmp(env, 1) != 0) {
    return 1;
  }
  quire_active = &env;
  memcpy(dst, src, len);
  quire_active = NULL;
  return 0;
}

int quire_guard_selftest(void) {
  sigjmp_buf env;
  pthread_once(&quire_once, quire_install);
  if (sigsetjmp(env, 1) != 0) {
    return 1;
  }
  quire_active = &env;
  raise(SIGBUS);
  quire_active = NULL;
  return 0;
}
#endif
