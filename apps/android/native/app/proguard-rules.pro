# UniFFI / JNI keep rules staged for release minification. Keep R8 disabled
# until a minified release device smoke of the UniFFI bindings has passed.

-keepclasseswithmembernames class * {
    native <methods>;
}

-keep class uniffi.** { *; }
-keep class uniffi.harness_runtime.** { *; }
-keep class com.lingxi.code.** { *; }
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }

-dontwarn uniffi.**
-dontwarn com.sun.jna.**

# Optional activity descriptors also work with a legacy generated SDK.
-keepclassmembers class com.lingxi.code.bindings.client.SessionRowDto {
    java.lang.String getActivityJson();
}
-keepclassmembers class com.lingxi.code.bindings.android.RemoteSessionSummaryFfi {
    java.lang.String getActivityJson();
}
