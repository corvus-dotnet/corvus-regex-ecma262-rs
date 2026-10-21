/* What the C tools need of the platform, on POSIX and on Windows: threads with a stack of a given size, a mutex, a
 * monotonic clock, thread-local storage and the files of a directory. */
#ifndef CRE_TOOLS_PLATFORM_H
#define CRE_TOOLS_PLATFORM_H

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _MSC_VER
#define THREAD_LOCAL __declspec(thread)
#else
#define THREAD_LOCAL _Thread_local
#endif

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <process.h>

typedef HANDLE thread_t;
typedef CRITICAL_SECTION mutex_t;

typedef struct {
    void *(*fn)(void *);
    void *arg;
} thread_start_args;

static inline unsigned __stdcall thread_trampoline(void *p) {
    thread_start_args a = *(thread_start_args *)p;
    free(p);
    a.fn(a.arg);
    return 0;
}

static inline int thread_start(thread_t *t, void *(*fn)(void *), void *arg, size_t stack) {
    thread_start_args *a = malloc(sizeof *a);
    a->fn = fn;
    a->arg = arg;
    *t = (HANDLE)_beginthreadex(NULL, (unsigned)stack, thread_trampoline, a, STACK_SIZE_PARAM_IS_A_RESERVATION, NULL);
    return *t ? 0 : -1;
}

static inline void thread_join(thread_t t) {
    WaitForSingleObject(t, INFINITE);
    CloseHandle(t);
}

static inline void mutex_init(mutex_t *m) { InitializeCriticalSection(m); }
static inline void mutex_lock(mutex_t *m) { EnterCriticalSection(m); }
static inline void mutex_unlock(mutex_t *m) { LeaveCriticalSection(m); }

static inline double now_seconds(void) {
    static LARGE_INTEGER frequency;
    LARGE_INTEGER t;
    if (!frequency.QuadPart) QueryPerformanceFrequency(&frequency);
    QueryPerformanceCounter(&t);
    return (double)t.QuadPart / (double)frequency.QuadPart;
}

/* The names of the files in a directory: an array of malloc'd strings, *count of them. */
static inline char **list_directory(const char *dir, size_t *count) {
    char pattern[4096];
    snprintf(pattern, sizeof pattern, "%s\\*", dir);
    WIN32_FIND_DATAA found;
    HANDLE h = FindFirstFileA(pattern, &found);
    size_t n = 0, cap = 64;
    char **names = malloc(cap * sizeof *names);
    if (h != INVALID_HANDLE_VALUE) {
        do {
            if (found.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) continue;
            if (n == cap) names = realloc(names, (cap *= 2) * sizeof *names);
            names[n++] = _strdup(found.cFileName);
        } while (FindNextFileA(h, &found));
        FindClose(h);
    }
    *count = n;
    return names;
}
#else
#include <dirent.h>
#include <pthread.h>
#include <time.h>

typedef pthread_t thread_t;
typedef pthread_mutex_t mutex_t;

static inline int thread_start(thread_t *t, void *(*fn)(void *), void *arg, size_t stack) {
    pthread_attr_t attr;
    pthread_attr_init(&attr);
    pthread_attr_setstacksize(&attr, stack);
    int r = pthread_create(t, &attr, fn, arg);
    pthread_attr_destroy(&attr);
    return r;
}

static inline void thread_join(thread_t t) { pthread_join(t, NULL); }
static inline void mutex_init(mutex_t *m) { pthread_mutex_init(m, NULL); }
static inline void mutex_lock(mutex_t *m) { pthread_mutex_lock(m); }
static inline void mutex_unlock(mutex_t *m) { pthread_mutex_unlock(m); }

static inline double now_seconds(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (double)t.tv_sec + (double)t.tv_nsec * 1e-9;
}

static inline char **list_directory(const char *dir, size_t *count) {
    DIR *d = opendir(dir);
    size_t n = 0, cap = 64;
    char **names = malloc(cap * sizeof *names);
    if (d) {
        struct dirent *e;
        while ((e = readdir(d)) != NULL) {
            if (e->d_name[0] == '.') continue;
            if (n == cap) names = realloc(names, (cap *= 2) * sizeof *names);
            names[n++] = strdup(e->d_name);
        }
        closedir(d);
    }
    *count = n;
    return names;
}
#endif

#endif
