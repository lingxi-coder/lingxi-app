#define _WIN32_WINNT 0x0601
#include <windows.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <wchar.h>

/* Isolated experiment, not a production process supervisor. */
enum { MAX_EVENTS = 16, ROOT_DEADLINE_MS = 20000, CHILD_DEADLINE_MS = 15000 };

typedef struct Shared {
    volatile LONG published;
    DWORD leaf_pid;
    volatile LONG nested_creates;
    volatile LONG nested_exits;
    volatile LONG nested_identity;
} Shared;

typedef struct Observed {
    DWORD pid;
    HANDLE process;
    bool breakpoint_seen;
    bool exit_seen;
    DWORD exit_code;
} Observed;

typedef BOOL (WINAPI *CompareObjectHandlesFn)(HANDLE, HANDLE);

static CompareObjectHandlesFn compare_function(void) {
    HMODULE library = GetModuleHandleW(L"kernelbase.dll");
    return library != NULL ? (CompareObjectHandlesFn)(void *)GetProcAddress(library, "CompareObjectHandles") : NULL;
}

static void fail_win(const char *stage) {
    fprintf(stderr, "error stage=%s win32=%lu\n", stage, (unsigned long)GetLastError());
}

static bool executable_path(wchar_t *path, size_t capacity) {
    DWORD n = GetModuleFileNameW(NULL, path, (DWORD)capacity);
    return n != 0 && n < capacity;
}

static bool spawn(const wchar_t *args, DWORD flags, PROCESS_INFORMATION *pi) {
    wchar_t path[32768], command[32768];
    STARTUPINFOW si;
    ZeroMemory(&si, sizeof(si));
    ZeroMemory(pi, sizeof(*pi));
    si.cb = sizeof(si);
    if (!executable_path(path, sizeof(path) / sizeof(path[0]))) {
        fail_win("executable_path");
        return false;
    }
    if (swprintf(command, sizeof(command) / sizeof(command[0]), L"\"%ls\" %ls", path, args) < 0) {
        fprintf(stderr, "error stage=format_command\n");
        return false;
    }
    if (!CreateProcessW(path, command, NULL, NULL, FALSE, flags, NULL, NULL, &si, pi)) {
        fail_win("CreateProcessW");
        return false;
    }
    return true;
}

static void close_debug_file(const DEBUG_EVENT *ev) {
    HANDLE h = NULL;
    if (ev->dwDebugEventCode == CREATE_PROCESS_DEBUG_EVENT)
        h = ev->u.CreateProcessInfo.hFile;
    else if (ev->dwDebugEventCode == LOAD_DLL_DEBUG_EVENT)
        h = ev->u.LoadDll.hFile;
    if (h != NULL && h != INVALID_HANDLE_VALUE) CloseHandle(h);
}

static int leaf(const wchar_t *event_name) {
    HANDLE event = OpenEventW(SYNCHRONIZE, FALSE, event_name);
    if (event == NULL) { fail_win("leaf_open_event"); return 10; }
    DWORD result = WaitForSingleObject(event, CHILD_DEADLINE_MS);
    CloseHandle(event);
    return result == WAIT_OBJECT_0 ? 0 : 11;
}

static int run_inner_debug_loop(PROCESS_INFORMATION *pi, Shared *shared, bool *process_managed, bool *thread_managed) {
    ULONGLONG deadline = GetTickCount64() + CHILD_DEADLINE_MS;
    bool breakpoint_seen = false, exited = false;
    int result = 0;
    while (!exited && GetTickCount64() < deadline) {
        DEBUG_EVENT ev;
        if (!WaitForDebugEvent(&ev, 50)) {
            if (GetLastError() == ERROR_SEM_TIMEOUT) continue;
            fail_win("inner_WaitForDebugEvent");
            return 12;
        }
        DWORD continuation = DBG_CONTINUE;
        if (ev.dwProcessId != pi->dwProcessId) result = 13;
        if (ev.dwDebugEventCode == CREATE_PROCESS_DEBUG_EVENT) {
            *process_managed = ev.u.CreateProcessInfo.hProcess == pi->hProcess;
            *thread_managed = ev.u.CreateProcessInfo.hThread == pi->hThread;
            CompareObjectHandlesFn compare = compare_function();
            if (compare != NULL && compare(ev.u.CreateProcessInfo.hProcess, pi->hProcess))
                InterlockedExchange(&shared->nested_identity, 1);
            InterlockedIncrement(&shared->nested_creates);
        }
        if (ev.dwDebugEventCode == EXIT_PROCESS_DEBUG_EVENT) {
            InterlockedIncrement(&shared->nested_exits);
            exited = true;
            if (ev.u.ExitProcess.dwExitCode != 0) result = 14;
        }
        if (ev.dwDebugEventCode == EXCEPTION_DEBUG_EVENT) {
            DWORD code = ev.u.Exception.ExceptionRecord.ExceptionCode;
            if (code == EXCEPTION_BREAKPOINT && !breakpoint_seen) breakpoint_seen = true;
            else continuation = DBG_EXCEPTION_NOT_HANDLED;
        }
        close_debug_file(&ev);
        if (!ContinueDebugEvent(ev.dwProcessId, ev.dwThreadId, continuation)) {
            fail_win("inner_ContinueDebugEvent");
            return 15;
        }
    }
    if (!exited) return 16;
    return result;
}

static int root(const wchar_t *map_name, const wchar_t *event_name, bool nested) {
    HANDLE map = OpenFileMappingW(FILE_MAP_ALL_ACCESS, FALSE, map_name);
    if (map == NULL) { fail_win("root_open_mapping"); return 20; }
    Shared *shared = (Shared *)MapViewOfFile(map, FILE_MAP_ALL_ACCESS, 0, 0, sizeof(*shared));
    if (shared == NULL) { fail_win("root_map"); CloseHandle(map); return 21; }
    wchar_t args[512];
    int rc = 0;
    PROCESS_INFORMATION pi;
    HANDLE leaf_wait = NULL;
    bool process_managed = false, thread_managed = false;
    if (swprintf(args, sizeof(args) / sizeof(args[0]), L"--leaf %ls", event_name) < 0 ||
        !spawn(args, nested ? DEBUG_PROCESS : 0, &pi)) {
        rc = 22;
        goto done;
    }
    if (!DuplicateHandle(GetCurrentProcess(), pi.hProcess, GetCurrentProcess(), &leaf_wait, 0, FALSE, DUPLICATE_SAME_ACCESS)) {
        fail_win("root_duplicate_leaf");
        TerminateProcess(pi.hProcess, 125);
        CloseHandle(pi.hProcess);
        CloseHandle(pi.hThread);
        rc = 25;
        goto done;
    }
    shared->leaf_pid = pi.dwProcessId;
    InterlockedExchange(&shared->published, 1);
    if (nested) rc = run_inner_debug_loop(&pi, shared, &process_managed, &thread_managed);
    DWORD wait = WaitForSingleObject(leaf_wait, CHILD_DEADLINE_MS);
    if (wait != WAIT_OBJECT_0) {
        if (rc == 0) rc = 23;
    } else {
        DWORD exit_code = STILL_ACTIVE;
        if (!GetExitCodeProcess(leaf_wait, &exit_code) || exit_code != 0) {
            if (rc == 0) rc = 24;
        }
    }
    CloseHandle(leaf_wait);
    if (!process_managed) CloseHandle(pi.hProcess);
    if (!thread_managed) CloseHandle(pi.hThread);
done:
    UnmapViewOfFile(shared);
    CloseHandle(map);
    return rc;
}

static Observed *find_observed(Observed *events, size_t count, DWORD pid) {
    for (size_t i = 0; i < count; ++i)
        if (events[i].pid == pid) return &events[i];
    return NULL;
}

static int run_probe(bool nested) {
    int rc = 2;
    HANDLE job = NULL, map = NULL, release = NULL, leaf_handle = NULL, root_handle = NULL;
    Shared *shared = NULL;
    PROCESS_INFORMATION root_pi;
    ZeroMemory(&root_pi, sizeof(root_pi));
    Observed events[MAX_EVENTS];
    ZeroMemory(events, sizeof(events));
    size_t event_count = 0;
    bool root_exited = false, leaf_enrolled = false, root_original_managed = false, thread_original_managed = false;
    bool event_pending = false, event_file_closed = false;
    DEBUG_EVENT pending;
    wchar_t map_name[128], event_name[128], args[512];
    DWORD pid = GetCurrentProcessId();
    unsigned long long nonce = (unsigned long long)GetTickCount64();
    swprintf(map_name, 128, L"Local\\lx-job-probe-map-%lu-%llu", (unsigned long)pid, nonce);
    swprintf(event_name, 128, L"Local\\lx-job-probe-release-%lu-%llu", (unsigned long)pid, nonce);
    printf("probe mode=%s supervisor_pid=%lu\n", nested ? "nested" : "normal", (unsigned long)pid);
    fflush(stdout);

    job = CreateJobObjectW(NULL, NULL);
    if (job == NULL) { fail_win("CreateJobObjectW"); goto cleanup; }
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION limit;
    ZeroMemory(&limit, sizeof(limit));
    limit.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if (!SetInformationJobObject(job, JobObjectExtendedLimitInformation, &limit, sizeof(limit))) {
        fail_win("set_kill_on_close"); goto cleanup;
    }
    map = CreateFileMappingW(INVALID_HANDLE_VALUE, NULL, PAGE_READWRITE, 0, sizeof(Shared), map_name);
    if (map == NULL) { fail_win("create_mapping"); goto cleanup; }
    if (GetLastError() == ERROR_ALREADY_EXISTS) { fprintf(stderr, "error stage=mapping_collision\n"); goto cleanup; }
    shared = (Shared *)MapViewOfFile(map, FILE_MAP_ALL_ACCESS, 0, 0, sizeof(*shared));
    if (shared == NULL) { fail_win("map_shared"); goto cleanup; }
    ZeroMemory(shared, sizeof(*shared));
    release = CreateEventW(NULL, TRUE, FALSE, event_name);
    if (release == NULL) { fail_win("create_release_event"); goto cleanup; }
    if (GetLastError() == ERROR_ALREADY_EXISTS) { fprintf(stderr, "error stage=event_collision\n"); goto cleanup; }
    if (swprintf(args, sizeof(args) / sizeof(args[0]), L"--root %ls %ls %d", map_name, event_name, nested ? 1 : 0) < 0) {
        fprintf(stderr, "error stage=format_root\n"); goto cleanup;
    }
    if (!spawn(args, DEBUG_PROCESS | CREATE_SUSPENDED, &root_pi)) goto cleanup;
    if (!DuplicateHandle(GetCurrentProcess(), root_pi.hProcess, GetCurrentProcess(), &root_handle,
                         0, FALSE, DUPLICATE_SAME_ACCESS)) { fail_win("duplicate_root"); goto cleanup; }
    if (!AssignProcessToJobObject(job, root_pi.hProcess)) { fail_win("assign_root_to_job"); goto cleanup; }
    if (ResumeThread(root_pi.hThread) == (DWORD)-1) { fail_win("resume_root"); goto cleanup; }

    ULONGLONG deadline = GetTickCount64() + ROOT_DEADLINE_MS;
    while (!root_exited && GetTickCount64() < deadline) {
        if (!leaf_enrolled && InterlockedCompareExchange(&shared->published, 0, 0) == 1) {
            leaf_handle = OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, FALSE, shared->leaf_pid);
            if (leaf_handle == NULL) { fail_win("open_leaf"); goto cleanup; }
            BOOL member = FALSE;
            if (!IsProcessInJob(leaf_handle, job, &member)) { fail_win("leaf_membership"); goto cleanup; }
            DWORD wait = WaitForSingleObject(leaf_handle, 0);
            printf("leaf pid=%lu job_member=%d pre_release_wait=%lu\n", (unsigned long)shared->leaf_pid, member, (unsigned long)wait);
            if (!member || wait != WAIT_TIMEOUT) { fprintf(stderr, "error stage=leaf_not_live_member\n"); goto cleanup; }
            leaf_enrolled = true;
            if (!SetEvent(release)) { fail_win("release_leaf"); goto cleanup; }
        }
        DEBUG_EVENT ev;
        if (!WaitForDebugEvent(&ev, 50)) {
            if (GetLastError() == ERROR_SEM_TIMEOUT) continue;
            fail_win("outer_WaitForDebugEvent"); goto cleanup;
        }
        pending = ev;
        event_pending = true;
        event_file_closed = false;
        DWORD continuation = DBG_CONTINUE;
        Observed *entry = find_observed(events, event_count, ev.dwProcessId);
        if (ev.dwDebugEventCode == CREATE_PROCESS_DEBUG_EVENT) {
            if (entry != NULL || event_count == MAX_EVENTS) { fprintf(stderr, "error stage=unexpected_create\n"); goto cleanup; }
            entry = &events[event_count++];
            entry->pid = ev.dwProcessId;
            if (ev.dwProcessId == root_pi.dwProcessId) {
                root_original_managed = ev.u.CreateProcessInfo.hProcess == root_pi.hProcess;
                thread_original_managed = ev.u.CreateProcessInfo.hThread == root_pi.hThread;
            }
            if (!DuplicateHandle(GetCurrentProcess(), ev.u.CreateProcessInfo.hProcess, GetCurrentProcess(),
                                 &entry->process, 0, FALSE, DUPLICATE_SAME_ACCESS)) {
                fail_win("duplicate_debug_process"); goto cleanup;
            }
            DWORD wait = WaitForSingleObject(entry->process, 0);
            printf("outer_create pid=%lu process_handle_wait=%lu\n", (unsigned long)entry->pid, (unsigned long)wait);
            if (wait == WAIT_FAILED) { fail_win("debug_process_not_waitable"); goto cleanup; }
        } else if (ev.dwDebugEventCode == EXIT_PROCESS_DEBUG_EVENT) {
            if (entry == NULL) { fprintf(stderr, "error stage=exit_without_create\n"); goto cleanup; }
            entry->exit_seen = true;
            entry->exit_code = ev.u.ExitProcess.dwExitCode;
            printf("outer_exit pid=%lu exit=%lu held_handle_wait=%lu\n", (unsigned long)entry->pid,
                   (unsigned long)entry->exit_code, (unsigned long)WaitForSingleObject(entry->process, 0));
            if (ev.dwProcessId == root_pi.dwProcessId) root_exited = true;
        } else if (ev.dwDebugEventCode == EXCEPTION_DEBUG_EVENT) {
            DWORD code = ev.u.Exception.ExceptionRecord.ExceptionCode;
            if (entry != NULL && code == EXCEPTION_BREAKPOINT && !entry->breakpoint_seen) entry->breakpoint_seen = true;
            else continuation = DBG_EXCEPTION_NOT_HANDLED;
        }
        close_debug_file(&ev);
        event_file_closed = true;
        if (!ContinueDebugEvent(ev.dwProcessId, ev.dwThreadId, continuation)) {
            fail_win("outer_ContinueDebugEvent"); goto cleanup;
        }
        event_pending = false;
    }
    if (!root_exited || !leaf_enrolled) { fprintf(stderr, "error stage=deadline_or_no_leaf\n"); goto cleanup; }
    if (WaitForSingleObject(root_handle, 5000) != WAIT_OBJECT_0 ||
        WaitForSingleObject(leaf_handle, 5000) != WAIT_OBJECT_0) {
        fprintf(stderr, "error stage=exact_process_wait\n"); goto cleanup;
    }
    DWORD root_exit = STILL_ACTIVE, leaf_exit = STILL_ACTIVE;
    if (!GetExitCodeProcess(root_handle, &root_exit) ||
        !GetExitCodeProcess(leaf_handle, &leaf_exit) || root_exit != 0 || leaf_exit != 0) {
        fprintf(stderr, "error stage=process_exit_codes root=%lu leaf=%lu\n", (unsigned long)root_exit, (unsigned long)leaf_exit);
        goto cleanup;
    }
    Observed *root_event = find_observed(events, event_count, root_pi.dwProcessId);
    Observed *leaf_event = find_observed(events, event_count, shared->leaf_pid);
    if (event_count != (nested ? 1u : 2u) || root_event == NULL || !root_event->exit_seen ||
        (!nested && (leaf_event == NULL || !leaf_event->exit_seen)) || (nested && leaf_event != NULL)) {
        fprintf(stderr, "error stage=observed_debug_chain root=%d leaf=%d\n", root_event != NULL, leaf_event != NULL);
        goto cleanup;
    }
    CompareObjectHandlesFn compare = compare_function();
    if (compare == NULL) { fprintf(stderr, "error stage=CompareObjectHandles_unavailable\n"); goto cleanup; }
    if (!compare(root_event->process, root_handle) ||
        (leaf_event != NULL && !compare(leaf_event->process, leaf_handle))) {
        fprintf(stderr, "error stage=object_identity\n"); goto cleanup;
    }
    for (size_t i = 0; i < event_count; ++i) {
        if (WaitForSingleObject(events[i].process, 5000) != WAIT_OBJECT_0) {
            fprintf(stderr, "error stage=event_process_wait pid=%lu\n", (unsigned long)events[i].pid); goto cleanup;
        }
    }
    if (nested && (shared->nested_creates != 1 || shared->nested_exits != 1 || shared->nested_identity != 1)) {
        fprintf(stderr, "error stage=inner_debug_events creates=%ld exits=%ld same_object=%ld\n", shared->nested_creates, shared->nested_exits, shared->nested_identity);
        goto cleanup;
    }
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION accounting;
    ZeroMemory(&accounting, sizeof(accounting));
    if (!QueryInformationJobObject(job, JobObjectBasicAccountingInformation, &accounting, sizeof(accounting), NULL)) {
        fail_win("query_job_accounting"); goto cleanup;
    }
    printf("result mode=%s outer_creates=%zu nested_creates=%ld nested_exits=%ld nested_same_object=%ld total=%lu active=%lu root_wait=signaled leaf_wait=signaled\n",
           nested ? "nested" : "normal", event_count, shared->nested_creates, shared->nested_exits,
           shared->nested_identity, (unsigned long)accounting.TotalProcesses, (unsigned long)accounting.ActiveProcesses);
    rc = 0;

cleanup:
    /* Continue any outstanding debug event while terminating the private fixture. */
    if (job != NULL) TerminateJobObject(job, 125);
    if (root_handle == NULL) root_handle = root_pi.hProcess;
    if (root_handle != NULL && WaitForSingleObject(root_handle, 0) == WAIT_TIMEOUT)
        TerminateProcess(root_handle, 125);
    if (event_pending) {
        if (!event_file_closed) close_debug_file(&pending);
        ContinueDebugEvent(pending.dwProcessId, pending.dwThreadId, DBG_CONTINUE);
    }
    ULONGLONG cleanup_deadline = GetTickCount64() + 5000;
    while (root_handle != NULL && WaitForSingleObject(root_handle, 0) == WAIT_TIMEOUT && GetTickCount64() < cleanup_deadline) {
        DEBUG_EVENT ev;
        if (!WaitForDebugEvent(&ev, 50)) continue;
        if (ev.dwDebugEventCode == CREATE_PROCESS_DEBUG_EVENT && ev.dwProcessId == root_pi.dwProcessId) {
            root_original_managed = ev.u.CreateProcessInfo.hProcess == root_pi.hProcess;
            thread_original_managed = ev.u.CreateProcessInfo.hThread == root_pi.hThread;
        }
        close_debug_file(&ev);
        ContinueDebugEvent(ev.dwProcessId, ev.dwThreadId, DBG_CONTINUE);
    }
    if (root_handle != NULL && WaitForSingleObject(root_handle, 0) != WAIT_OBJECT_0)
        fprintf(stderr, "error stage=cleanup_root_not_signaled\n");
    if (leaf_handle != NULL && WaitForSingleObject(leaf_handle, 5000) != WAIT_OBJECT_0)
        fprintf(stderr, "error stage=cleanup_leaf_not_signaled\n");
    if (job != NULL) CloseHandle(job);
    if (root_handle != NULL && root_handle != root_pi.hProcess) CloseHandle(root_handle);
    if (root_pi.hProcess != NULL && !root_original_managed) CloseHandle(root_pi.hProcess);
    if (root_pi.hThread != NULL && !thread_original_managed) CloseHandle(root_pi.hThread);
    if (leaf_handle != NULL) CloseHandle(leaf_handle);
    for (size_t i = 0; i < event_count; ++i)
        if (events[i].process != NULL) CloseHandle(events[i].process);
    if (release != NULL) CloseHandle(release);
    if (shared != NULL) UnmapViewOfFile(shared);
    if (map != NULL) CloseHandle(map);
    return rc;
}

int wmain(int argc, wchar_t **argv) {
    if (argc == 3 && wcscmp(argv[1], L"--leaf") == 0) return leaf(argv[2]);
    if (argc == 5 && wcscmp(argv[1], L"--root") == 0) return root(argv[2], argv[3], wcscmp(argv[4], L"1") == 0);
    if (argc == 2 && wcscmp(argv[1], L"--normal") == 0) return run_probe(false);
    if (argc == 2 && wcscmp(argv[1], L"--nested") == 0) return run_probe(true);
    fprintf(stderr, "usage: probe.exe --normal | --nested\n");
    return 2;
}
