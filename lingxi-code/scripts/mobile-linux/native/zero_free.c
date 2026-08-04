/*
 * zero_free.so - the LD_PRELOAD object the iSH ARM64 emulator demands for Node.
 *
 * OpenMinis unconditionally injects `LD_PRELOAD=/lib/zero_free.so` into every
 * `node` exec, from four separate sites:
 *     deps/ish/kernel/exec.c:990        (skips if LD_PRELOAD is already set)
 *     deps/ish/xX_main_Xx.h:244         (unconditional append)
 *     src/ios/iSH/ISHKernel.m:791       (unconditional append)
 *     src/ios/iSH/ISHShellExecutor.m:408 (unconditional append)
 * The upstream snapshot never ships the library, so musl's loader aborts and
 * Node cannot start at all. Building the object the injection already names
 * fixes that without patching the vendored snapshot.
 *
 * WHY THIS IS INTENTIONALLY EMPTY
 * ------------------------------
 * The upstream rationale (deps/ish/kernel/exec.c:985) describes a shim that
 * zeroes blocks >= 4096 bytes on malloc and free, because V8's Zone allocator
 * recycles freed memory without clearing it and stale pointers then surface
 * under emulation.
 *
 * Implementing that faithfully crashes Node, and the reason is not the zeroing.
 * Measured against Alpine 3.24.1 / Node v24.18.1 / musl 1.2.6 on aarch64:
 *
 *   - node runs clean with no LD_PRELOAD at all;
 *   - a shim zeroing on both malloc and free  -> SIGSEGV;
 *   - a shim zeroing only on malloc           -> SIGSEGV;
 *   - a *pure passthrough* malloc that only forwards via dlsym -> SIGSEGV;
 *   - a constructor-resolved variant reports "malloc called before ctor".
 *
 * So interposing malloc at all is what breaks: musl calls malloc before a
 * preloaded object's constructors run, and resolving the real allocator with
 * dlsym() from inside that first malloc re-enters a dynamic linker that is not
 * yet ready. (Zeroing on free is independently unsafe here too -- musl's
 * mallocng keeps in-band bookkeeping in the slot tail that malloc_usable_size()
 * reports as usable, so clearing that range corrupts the allocator.)
 *
 * Interposing the allocator safely would mean shipping a complete
 * malloc/free/calloc/realloc implementation with its own pre-constructor arena
 * -- a real allocator, not a shim -- to work around a hazard whose necessity we
 * cannot even observe from a native aarch64 rootfs, since the Zone-allocator
 * corruption it targets is a property of iSH's *emulator*.
 *
 * This object therefore satisfies the loader and interposes nothing, which is
 * strictly better than the status quo of a missing file. If on-device testing
 * under iSH shows V8 instability that zeroing would address, the fix belongs in
 * a real allocator implementation or in the injection sites themselves -- not
 * in a partial interposer that reliably segfaults before Node prints a line.
 */

/*
 * A shared object with no symbols is still a valid preload target. The marker
 * gives the file a defined symbol so it is trivially greppable on device and
 * cannot be mistaken for a truncated or corrupt build artifact.
 */
__attribute__((visibility("default")))
const char lingxi_zero_free_build[] =
    "lingxi zero_free.so: intentional no-op; see native/zero_free.c";
