/**
 * Low-level syscalls for Qi async runtime
 * 
 * This file provides platform-specific syscall wrappers for the async runtime.
 * It focuses on sleep, timing, and basic I/O operations that are needed by
 * the Rust async executor.
 */

#include <stdint.h>
#include <time.h>
#include <errno.h>

#ifdef _WIN32
#include <windows.h>
#else
#include <unistd.h>
#include <sys/time.h>
#include <sched.h>
#endif

/**
 * Sleep for the specified number of milliseconds
 * 
 * @param ms Number of milliseconds to sleep
 * @return 0 on success, -1 on error
 */
int qi_async_sys_sleep_ms(int ms) {
    if (ms < 0) {
        return -1;
    }

#ifdef _WIN32
    Sleep((DWORD)ms);
    return 0;
#else
    struct timespec req;
    req.tv_sec = ms / 1000;
    req.tv_nsec = (ms % 1000) * 1000000;
    
    if (nanosleep(&req, NULL) == -1) {
        return -1;
    }
    return 0;
#endif
}

/**
 * Get monotonic time in nanoseconds
 * 
 * This provides a monotonically increasing time value suitable for
 * measuring elapsed time, independent of system clock changes.
 * 
 * @return Time in nanoseconds, or -1 on error
 */
int64_t qi_async_sys_monotonic_time_ns(void) {
#ifdef _WIN32
    // Windows: Use QueryPerformanceCounter
    LARGE_INTEGER frequency, counter;
    
    if (!QueryPerformanceFrequency(&frequency)) {
        return -1;
    }
    
    if (!QueryPerformanceCounter(&counter)) {
        return -1;
    }
    
    // Convert to nanoseconds
    return (int64_t)((counter.QuadPart * 1000000000LL) / frequency.QuadPart);
    
#elif defined(__APPLE__)
    // macOS: Use clock_gettime with CLOCK_MONOTONIC
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) {
        return -1;
    }
    return (int64_t)(ts.tv_sec * 1000000000LL + ts.tv_nsec);
    
#else
    // Linux and other POSIX systems
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) {
        return -1;
    }
    return (int64_t)(ts.tv_sec * 1000000000LL + ts.tv_nsec);
#endif
}

/**
 * Get the current CPU time for the process in nanoseconds
 * 
 * @return CPU time in nanoseconds, or -1 on error
 */
int64_t qi_async_sys_cpu_time_ns(void) {
#ifdef _WIN32
    FILETIME creation_time, exit_time, kernel_time, user_time;
    
    if (!GetProcessTimes(GetCurrentProcess(), &creation_time, &exit_time, 
                         &kernel_time, &user_time)) {
        return -1;
    }
    
    // Convert FILETIME to nanoseconds (FILETIME is in 100ns units)
    ULARGE_INTEGER kernel, user;
    kernel.LowPart = kernel_time.dwLowDateTime;
    kernel.HighPart = kernel_time.dwHighDateTime;
    user.LowPart = user_time.dwLowDateTime;
    user.HighPart = user_time.dwHighDateTime;
    
    return (int64_t)((kernel.QuadPart + user.QuadPart) * 100);
    
#else
    // POSIX systems
    struct timespec ts;
    if (clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &ts) != 0) {
        return -1;
    }
    return (int64_t)(ts.tv_sec * 1000000000LL + ts.tv_nsec);
#endif
}

/**
 * Yield the current thread to the scheduler
 * 
 * @return 0 on success, -1 on error
 */
int qi_async_sys_yield(void) {
#ifdef _WIN32
    SwitchToThread();
    return 0;
#else
    if (sched_yield() == -1) {
        return -1;
    }
    return 0;
#endif
}

/**
 * Get the number of available CPU cores
 * 
 * @return Number of CPU cores, or -1 on error
 */
int qi_async_sys_cpu_count(void) {
#ifdef _WIN32
    SYSTEM_INFO sys_info;
    GetSystemInfo(&sys_info);
    return (int)sys_info.dwNumberOfProcessors;
#else
    long nprocs = sysconf(_SC_NPROCESSORS_ONLN);
    if (nprocs == -1) {
        return -1;
    }
    return (int)nprocs;
#endif
}
