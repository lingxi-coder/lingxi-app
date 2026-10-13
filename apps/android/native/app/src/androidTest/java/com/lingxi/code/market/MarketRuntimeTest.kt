package com.lingxi.code.market

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.webkit.WebViewFeature
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import kotlinx.coroutines.*
import kotlin.coroutines.resume
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject

/** Run on an emulator/device with modern Android System WebView. No product UI dependency. */
@RunWith(AndroidJUnit4::class)
class MarketRuntimeTest {
    @Test fun localResponseHasCspRangeAndHead() {
        val root=java.nio.file.Files.createTempDirectory("market-runtime").toFile()
        try {
            File(root,"index.html").writeText("0123456789")
            val response=MarketWebRuntime.serve(root,"index.html","bytes=2-5")
            assertEquals(206,response.statusCode); assertEquals("2345",response.data.bufferedReader().readText())
            assertTrue(response.responseHeaders["Content-Security-Policy"]!!.contains("worker-src 'none'"))
            assertEquals("bytes 2-5/10",response.responseHeaders["Content-Range"])
            val head=MarketWebRuntime.serve(root,"index.html",null,true)
            assertEquals(0,head.data.readBytes().size); assertEquals("10",head.responseHeaders["Content-Length"])
            assertEquals(404,MarketWebRuntime.serve(root,"../outside",null).statusCode)
        } finally { root.deleteRecursively() }
    }
    @Test fun providerSupportsRequiredIsolation() {
        assertTrue("Use a current Android System WebView",WebViewFeature.isFeatureSupported(WebViewFeature.MULTI_PROFILE))
        assertTrue("Use a current Android System WebView",WebViewFeature.isFeatureSupported(WebViewFeature.DELETE_BROWSING_DATA))
    }
    @Test fun isolatedStoragePersistsAcrossUpdateAndResetsOnReinstall()=runBlocking {
        val context=InstrumentationRegistry.getInstrumentation().targetContext
        val root=File(context.filesDir,"market-smoke-${java.util.UUID.randomUUID()}")
        val runtime=MarketWebRuntime(context,root)
        val client=object: MarketClient("https://market.test/api/v1") {
            var sequence=1L
            override suspend fun detail(appId: String)=MarketApp(appId,appId,"","",listOf("android"),null,null,"")
            override suspend fun request(path: String,body: JSONObject?): JSONObject {
                val parts=path.split('/'); val app=parts[2]; val version=parts[4]
                return JSONObject().put("data",JSONObject().put("appId",app).put("versionId",version).put("sequence",sequence).put("artifact",JSONObject().put("format","html").put("sizeBytes",1).put("sha256","0".repeat(64)).put("downloadUrl","https://market.test/file")))
            }
            override suspend fun download(d: MarketDistribution,target: File,progress: (Long,Long)->Unit) {
                target.writeText("<html><script type='module'>window.moduleLoaded=true</script></html>")
            }
        }
        val repository=MarketRepository(root,client,runtime); runtime.repository=repository
        suspend fun eval(web: android.webkit.WebView,script: String): String=withContext(Dispatchers.Main.immediate) {
            suspendCancellableCoroutine { c -> web.evaluateJavascript(script) { if(c.isActive) c.resume(it) } }
        }
        suspend fun ready(web: android.webkit.WebView) { withTimeout(15000) { while(eval(web,"window.moduleLoaded === true")!="true") delay(100) } }
        try {
            repository.install("a","v1"); repository.install("b","v1")
            val a=runtime.open("a"); ready(a); eval(a,"localStorage.setItem('test','persistent')")
            val b=runtime.open("b"); ready(b); assertEquals("null",eval(b,"localStorage.getItem('test')"))
            runtime.close("a"); val reopened=runtime.open("a"); ready(reopened)
            assertEquals("\"persistent\"",eval(reopened,"localStorage.getItem('test')"))
            client.sequence=2; repository.install("a","v2",true)
            val updated=runtime.open("a"); ready(updated); assertEquals("\"persistent\"",eval(updated,"localStorage.getItem('test')"))
            repository.uninstall("a"); repository.install("a","v3")
            val fresh=runtime.open("a"); ready(fresh); assertEquals("null",eval(fresh,"localStorage.getItem('test')"))
            repository.uninstall("a"); repository.uninstall("b")
        } finally { runtime.close("a"); runtime.close("b"); root.deleteRecursively() }
    }

}
