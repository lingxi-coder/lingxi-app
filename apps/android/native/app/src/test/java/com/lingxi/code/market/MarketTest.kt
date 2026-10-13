package com.lingxi.code.market

import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.io.File
import java.nio.file.Files
import java.security.MessageDigest
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream

@OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
class MarketTest {
    private fun root()=Files.createTempDirectory("market-test").toFile()
    private fun html()="<html><script>localStorage.x=1</script></html>".toByteArray()
    private fun digest(b: ByteArray)=MessageDigest.getInstance("SHA-256").digest(b).joinToString("") { "%02x".format(it) }
    private fun zip(vararg names: String): File=File(root(),"package.zip").also { file -> ZipOutputStream(file.outputStream()).use { out -> names.forEach { out.putNextEntry(ZipEntry(it)); out.write(html()); out.closeEntry() } } }
    private class Runtime: MarketRuntime {
        val closed=mutableListOf<String>(); var failClear=false
        override suspend fun close(appId: String) { closed+=appId }
        override suspend fun clearData(installationId: String) { if(failClear) error("cleanup failed") }
        override suspend fun recoverProfiles(tombstones: Set<String>)=emptySet<String>()
    }
    private class Client: MarketClient("https://market.test/api/v1") {
        var sequence=1L; var invalid=false
        override suspend fun detail(appId: String)=MarketApp(appId,appId,"","",listOf("android"),null,null,"")
        override suspend fun request(path: String,body: JSONObject?): JSONObject = if(path.endsWith("distribution")) JSONObject().put("data",JSONObject().put("appId","a").put("versionId",path.split('/').let { it[it.size-2] }).put("sequence",sequence).put("artifact",JSONObject().put("format",if(invalid) "exe" else "html").put("sizeBytes",1).put("sha256","0".repeat(64)).put("downloadUrl","https://market.test/file"))) else error("unsupported")
        override suspend fun download(d: MarketDistribution,target: File,progress: (Long,Long)->Unit) { target.writeBytes("test".toByteArray()); progress(4,4) }
    }
    @Test fun htmlAndZipInstallEntry()=runTest {
        val output=root(); val artifact=File(root(),"artifact").apply { writeBytes(html()) }; MarketRepository.extract(artifact,"html",output); assertArrayEquals(html(),File(output,"index.html").readBytes())
        val zipOut=root(); MarketRepository.extract(zip("index.html","assets/main.js"),"zip",zipOut); assertTrue(File(zipOut,"assets/main.js").exists())
    }
    @Test fun traversalAndConflictsAndMissingIndexRejected()=runTest {
        for(names in listOf(arrayOf("../index.html"),arrayOf("/index.html"),arrayOf("index.html","assets","assets/main.js"),arrayOf("main.js"))) {
            try { MarketRepository.extract(zip(*names),"zip",root()); fail("accepted ${names.toList()}") } catch(_: MarketException) { }
        }
    }
    @Test fun updateKeepsIdentityAndFailedUpdateKeepsRegistry()=runTest {
        val runtime=Runtime(); val client=Client(); val r=MarketRepository(root(),client,runtime)
        val first=r.install("a","v1"); client.sequence=2; val second=r.install("a","v2",true)
        assertEquals(first.installationId,second.installationId); assertNotEquals(first.directory,second.directory)
        client.sequence=3; client.invalid=true
        try { r.install("a","v3",true); fail() } catch(_: MarketException) { }
        assertEquals(second,r.installed().single()); assertEquals(2,runtime.closed.size)
    }
    @Test fun cleanupFailureBlocksOpenThenColdRecoveryRemovesPendingInstall()=runTest {
        val dir=root(); val runtime=Runtime(); val client=Client(); val r=MarketRepository(dir,client,runtime); val app=r.install("a","v1")
        runtime.failClear=true; try { r.uninstall("a"); fail() } catch(_: IllegalStateException) { }
        assertFalse(r.canOpen(app)); assertEquals(1,r.installed().size)
        val recovered=MarketRepository(dir,client,Runtime()); assertTrue(recovered.installed().isEmpty()); assertFalse(File(dir,"versions/${app.directory}").exists())
    }
    @Test fun staleQueryDoesNotReplaceCurrentCatalog()=runTest {
        val client=object: MarketClient("https://market.test") {
            override suspend fun catalog(query: String,cursor: String?): MarketPage<MarketApp> { withContext(NonCancellable) { if(query=="old") delay(100) }; return MarketPage(listOf(MarketApp(query,query,"","",listOf("android"),null,null,"")),null) }
        }
        val store=MarketStore(client,MarketRepository(root(),client,Runtime()),this)
        store.query("old"); runCurrent(); store.query("new"); advanceUntilIdle(); assertEquals("new",store.state.value.apps.single().id)
    }
    @Test fun platformSortAndPaginationAndUpdateStatuses()=runTest {
        val client=object: MarketClient("https://market.test") {
            val paths=mutableListOf<String>()
            override suspend fun request(path: String,body: JSONObject?): JSONObject {
                paths+=path
                if(path=="/updates/check") return JSONObject("""{"data":[{"appId":"a","status":"up_to_date"},{"appId":"b","status":"update_available","currentVersionId":"v2","currentSequence":2},{"appId":"c","status":"unknown_version"},{"appId":"d","status":"unavailable"}]}""")
                return JSONObject("""{"data":[],"pagination":{"nextCursor":"next"}}""")
            }
        }
        assertEquals("next",client.catalog().nextCursor); client.catalog("hello world","opaque")
        assertTrue(client.paths[0].contains("platform=android")); assertTrue(client.paths[0].contains("sort=updated")); assertTrue(client.paths[1].contains("sort=name")); assertTrue(client.paths[1].contains("q=hello+world")); assertTrue(client.paths[1].contains("cursor=opaque"))
        assertEquals(UpdateStatus.entries,client.updates(listOf(InstalledApp("x","a","v1",1,"u","d"))).map { it.status })
    }
    @Test fun downloadValidatesActualSizeAndDigest()=runTest {
        val bytes=html()
        val client=object: MarketClient("https://market.test") {
            override fun connection(address: String)=object: java.net.HttpURLConnection(java.net.URL(address)) {
                override fun disconnect() {} ; override fun usingProxy()=false; override fun connect() {}
                override fun getResponseCode()=200
                override fun getInputStream()=java.io.ByteArrayInputStream(bytes)
            }
        }
        val distribution=MarketDistribution("a","v1",1,"html",bytes.size.toLong(),digest(bytes),"https://market.test/download")
        client.download(distribution,File(root(),"artifact")) { _,_-> }
        for(d in listOf(distribution.copy(sizeBytes=bytes.size-1L),distribution.copy(sha256="0".repeat(64)))) {
            try { client.download(d,File(root(),"artifact")) { _,_-> }; fail() } catch(_: MarketException) { }
        }
    }
    @Test fun coldStartRemovesOrphanVersionsAndStaging()=runTest {
        val dir=root(); File(dir,"staging/stale").mkdirs(); File(dir,"versions/orphan").mkdirs()
        MarketRepository(dir,Client(),Runtime()).initialize()
        assertFalse(File(dir,"staging").exists()); assertFalse(File(dir,"versions/orphan").exists())
    }

    @Test fun entryCountAndSymlinkRejected()=runTest {
        try { MarketRepository.extract(zip(*(0..2000).map { "$it.html" }.toTypedArray()),"zip",root()); fail() } catch(_: MarketException) { }
        val file=File(root(),"symlink.zip")
        org.apache.commons.compress.archivers.zip.ZipArchiveOutputStream(file).use { out ->
            val entry=org.apache.commons.compress.archivers.zip.ZipArchiveEntry("index.html")
            entry.setUnixMode(40960 or 511); out.putArchiveEntry(entry); out.write("../outside".toByteArray()); out.closeArchiveEntry()
        }
        try { MarketRepository.extract(file,"zip",root()); fail() } catch(_: MarketException) { }
    }
    @Test fun failedRegistryCommitRetainsPriorRecord()=runTest {
        val dir=root(); val client=Client(); val repository=MarketRepository(dir,client,Runtime())
        val original=repository.install("a","v1"); client.sequence=2
        File(dir,"registry.json").delete(); File(dir,"registry.json").mkdir()
        try { repository.install("a","v2",true); fail() } catch(_: java.io.IOException) { }
        assertEquals(original,repository.installed().single()); assertTrue(File(dir,"versions/${original.directory}/index.html").isFile)
    }
    @Test fun storePaginationDedupesAndSurfacesErrors()=runTest {
        val app=MarketApp("a","A","","",listOf("android"),null,null,"")
        val client=object: MarketClient("https://market.test") {
            override suspend fun catalog(query: String,cursor: String?): MarketPage<MarketApp> {
                if(query=="fail") throw MarketException("SERVICE_UNAVAILABLE")
                return MarketPage(listOf(app),if(cursor==null) "next" else null)
            }
        }
        val store=MarketStore(client,MarketRepository(root(),client,Runtime()),this)
        store.query(""); advanceUntilIdle(); store.loadNext(); advanceUntilIdle(); assertEquals(1,store.state.value.apps.size)
        store.query("fail"); advanceUntilIdle(); assertEquals("SERVICE_UNAVAILABLE",store.state.value.error); assertFalse(store.state.value.loading)
    }
    @Test fun pendingUninstallCannotUpdate()=runTest {
        val runtime=Runtime(); val client=Client(); val r=MarketRepository(root(),client,runtime)
        r.install("a","v1"); runtime.failClear=true; try { r.uninstall("a") } catch(_: IllegalStateException) { }
        client.sequence=2
        try { r.install("a","v2",true); fail() } catch(e: MarketException) { assertEquals("UNINSTALL_PENDING",e.code) }
    }

    @Test fun crcCorruptionRejected()=runTest {
        val file=File(root(),"corrupt.zip"); val payload="plain-payload".toByteArray()
        ZipOutputStream(file.outputStream()).use { out ->
            val entry=ZipEntry("index.html"); entry.method=ZipEntry.STORED; entry.size=payload.size.toLong(); entry.compressedSize=entry.size
            entry.crc=java.util.zip.CRC32().apply { update(payload) }.value
            out.putNextEntry(entry); out.write(payload); out.closeEntry()
        }
        val bytes=file.readBytes(); val position=(0..bytes.size-payload.size).first { offset -> payload.indices.all { bytes[offset+it]==payload[it] } }
        bytes[position]=(bytes[position].toInt() xor 1).toByte(); file.writeBytes(bytes)
        try { MarketRepository.extract(file,"zip",root()); fail() } catch(e: MarketException) { assertEquals("ZIP_CRC_MISMATCH",e.code) }
    }
    @Test fun sameAppConcurrentInstallsSerialize()=runTest {
        val repository=MarketRepository(root(),Client(),Runtime())
        val results=listOf(async { runCatching { repository.install("a","v1") } },async { runCatching { repository.install("a","v1") } }).awaitAll()
        assertEquals(1,results.count { it.isSuccess }); assertEquals(1,repository.installed().size)
    }

}
