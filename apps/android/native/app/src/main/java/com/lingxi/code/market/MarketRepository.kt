package com.lingxi.code.market

import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.io.FileOutputStream
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.apache.commons.compress.archivers.zip.ZipFile
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.UUID
import java.util.zip.CRC32
import kotlin.coroutines.coroutineContext

data class InstalledApp(val marketSource: String, val appId: String, val versionId: String, val sequence: Long, val installationId: String, val directory: String)
interface MarketRuntime {
    suspend fun close(appId: String)
    suspend fun clearData(installationId: String)
    suspend fun recoverProfiles(tombstones: Set<String>): Set<String>
}
/** One repository instance per process; its committed registry is the installed source of truth. */
class MarketRepository(val root: File, val client: MarketClient, private val runtime: MarketRuntime) {
    private val registry=File(root,"registry.json")
    private val global=Mutex(); private val appLocks=java.util.concurrent.ConcurrentHashMap<String,Mutex>()
    private var records=emptyList<InstalledApp>(); private var tombstones=emptySet<String>(); private var initialized=false
    suspend fun initialize() = global.withLock { withContext(Dispatchers.IO) {
        if(initialized) return@withContext
        root.mkdirs()
        if(registry.exists()) {
            val j=registry.inputStream().use { JSONObject(it.readBytes().toString(Charsets.UTF_8)) }; val a=j.getJSONArray("apps")
            records=(0 until a.length()).map { a.getJSONObject(it).let { x -> InstalledApp(x.getString("marketSource"),x.getString("appId"),x.getString("versionId"),x.getLong("sequence"),x.getString("installationId"),x.getString("directory")) } }
            val t=j.optJSONArray("tombstones") ?: JSONArray(); tombstones=(0 until t.length()).map(t::getString).toSet()
        }
        val pending=tombstones
        tombstones=runtime.recoverProfiles(pending)
        records=records.filter { it.installationId !in (pending-tombstones) }
        write(records,tombstones)
        File(root,"staging").deleteRecursively()
        val retained=records.map { it.directory }.toSet()
        File(root,"versions").listFiles()?.filter { it.name !in retained }?.forEach { it.deleteRecursively() }
        initialized=true
    } }
    suspend fun installed(): List<InstalledApp> { initialize(); return global.withLock { records.toList() } }
    private fun write(next: List<InstalledApp>, deleted: Set<String>) {
        val j=JSONObject().put("apps",JSONArray(next.map { JSONObject().put("marketSource",it.marketSource).put("appId",it.appId).put("versionId",it.versionId).put("sequence",it.sequence).put("installationId",it.installationId).put("directory",it.directory) })).put("tombstones",JSONArray(deleted.toList()))
        val temporary=File(root,"registry.tmp")
        FileOutputStream(temporary).use { it.write(j.toString().toByteArray()); it.fd.sync() }
        Files.move(temporary.toPath(),registry.toPath(),StandardCopyOption.ATOMIC_MOVE,StandardCopyOption.REPLACE_EXISTING)
    }
    suspend fun install(appId: String, versionId: String, updating: Boolean = false, progress: (Long,Long)->Unit = { _,_-> }, committing: ()->Unit = {}): InstalledApp {
        initialize()
        return appLocks.getOrPut(appId) { Mutex() }.withLock { withContext(Dispatchers.IO) {
            val prior=global.withLock { records.find { it.appId==appId } }
            if(prior!=null && global.withLock { prior.installationId in tombstones }) throw MarketException("UNINSTALL_PENDING")
            if(prior!=null && prior.marketSource!=client.apiRoot) throw MarketException("MARKET_SOURCE_MISMATCH")
            if(updating != (prior!=null)) throw MarketException(if(updating) "NOT_INSTALLED" else "ALREADY_INSTALLED")
            if("android" !in client.detail(appId).platforms) throw MarketException("PLATFORM_UNSUPPORTED")
            val d=client.distribution(appId,versionId)
            if(prior!=null && d.sequence<=prior.sequence) throw MarketException("NOT_NEWER")
            val stage=File(root,"staging/${UUID.randomUUID()}"); stage.mkdirs()
            try {
                val artifact=File(stage,"artifact"); client.download(d,artifact,progress)
                val contents=File(stage,"contents"); contents.mkdirs(); extract(artifact,d.format,contents)
                coroutineContext.ensureActive()
                // Commit is deliberately non-cancellable once the validated replacement is ready.
                withContext(kotlinx.coroutines.NonCancellable) {
                    committing(); runtime.close(appId)
                    val directory=UUID.randomUUID().toString(); val dest=File(root,"versions/$directory"); dest.parentFile!!.mkdirs()
                    if(!contents.renameTo(dest)) throw MarketException("COMMIT_FAILED")
                    val record=InstalledApp(client.apiRoot,appId,versionId,d.sequence,prior?.installationId ?: UUID.randomUUID().toString(),directory)
                    try { global.withLock { val next=records.filter { it.appId!=appId }+record; write(next,tombstones); records=next } } catch(e: Throwable) { dest.deleteRecursively(); throw e }
                    prior?.let { File(root,"versions/${it.directory}").deleteRecursively() }; record
                }
            } finally { stage.deleteRecursively() }
        } }
    }
    suspend fun uninstall(appId: String) {
        initialize(); appLocks.getOrPut(appId) { Mutex() }.withLock {
            val item=global.withLock { records.find { it.appId==appId } } ?: return@withLock
            withContext(kotlinx.coroutines.NonCancellable) {
                runtime.close(appId)
                // Persist a deletion marker before clearing data; prevents reopening on a partial failure.
                global.withLock { val next=tombstones+item.installationId; write(records,next); tombstones=next }
                runtime.clearData(item.installationId)
                global.withLock { val next=records.filter { it.appId!=appId }; write(next,tombstones); records=next }
                withContext(Dispatchers.IO) { File(root,"versions/${item.directory}").deleteRecursively() }
            }
        }
    }
    suspend fun canOpen(item: InstalledApp): Boolean = global.withLock { item.installationId !in tombstones && item in records }
    suspend fun withAppLock(appId: String, block: suspend ()->Unit) = appLocks.getOrPut(appId) { Mutex() }.withLock { block() }
    companion object {
        internal suspend fun extract(artifact: File, format: String, output: File) {
            if(format=="html") { artifact.copyTo(File(output,"index.html")); return }
            if(format!="zip") throw MarketException("UNSUPPORTED_FORMAT")
            ZipFile.builder().setFile(artifact).get().use { zip ->
                val names=mutableSetOf<String>(); val files=mutableSetOf<String>(); var count=0; var expanded=0L
                val entries=zip.entries
                while(entries.hasMoreElements()) {
                    coroutineContext.ensureActive(); val e=entries.nextElement(); count++
                    if(count>2000 || e.isUnixSymlink || e.generalPurposeBit.usesEncryption() || e.method !in listOf(0,8) || !zip.canReadEntryData(e)) throw MarketException("UNSAFE_ZIP")
                    val raw=e.name.replace('\\','/'); val parts=raw.trimEnd('/').split('/')
                    if(raw.startsWith('/') || ':' in raw || parts.any { it.isEmpty() || it=="." || it==".." }) throw MarketException("UNSAFE_PATH")
                    val name=parts.joinToString("/")
                    if(!names.add(name) || parts.indices.drop(1).any { parts.take(it).joinToString("/") in files } || (!e.isDirectory && names.any { it.startsWith("$name/") })) throw MarketException("PATH_CONFLICT")
                    val target=File(output,name); if(e.isDirectory) { target.mkdirs(); continue }; files+=name; target.parentFile!!.mkdirs()
                    val crc=CRC32(); var size=0L
                    zip.getInputStream(e).use { input -> target.outputStream().use { out -> val b=ByteArray(32768); while(true) { coroutineContext.ensureActive(); val n=input.read(b); if(n<0) break; size+=n; expanded+=n; if(expanded>100L*1024*1024) throw MarketException("EXPANDED_TOO_LARGE"); crc.update(b,0,n); out.write(b,0,n) } } }
                    if(size!=e.size || crc.value!=e.crc) throw MarketException("ZIP_CRC_MISMATCH")
                }
            }
            if(!File(output,"index.html").isFile) throw MarketException("MISSING_INDEX")
        }
    }
}
