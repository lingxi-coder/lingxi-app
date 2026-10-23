#include <jni.h>
#include <stdint.h>

// Test APK only. Kept unoptimized with DWARF and GNU build ID for cloud validation.
__attribute__((noinline)) void CrashlyticsNdkVerificationLeaf(void) {
    *(volatile uint32_t *)(uintptr_t)0 = 0x295;
}

JNIEXPORT void JNICALL
Java_com_lingxi_code_diagnostics_CrashlyticsDeviceTest_crashNative(JNIEnv *env, jobject instance) {
    (void)env;
    (void)instance;
    CrashlyticsNdkVerificationLeaf();
}
