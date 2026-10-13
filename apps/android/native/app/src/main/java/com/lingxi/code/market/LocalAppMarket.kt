package com.lingxi.code.market

import android.content.Context
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import java.io.File

enum class OperationPhase { DOWNLOADING, COMMITTING, UNINSTALLING, COMPLETE, CANCELLED, FAILED }
data class MarketOperation(val phase: OperationPhase, val downloadedBytes: Long=0, val totalBytes: Long=0, val error: String?=null)
data class MarketState(val query: String="", val apps: List<MarketApp> = emptyList(), val cursor: String?=null, val loading: Boolean=false, val error: String?=null, val installed: List<InstalledApp> = emptyList(), val operations: Map<String,MarketOperation> = emptyMap(), val updates: List<MarketUpdate> = emptyList(), val runtimeError: String?=null)
class MarketStore(val client: MarketClient,val repository: MarketRepository,private val scope: CoroutineScope,private val capability: ()->String? = { null }) {
    private val mutable=MutableStateFlow(MarketState()); val state: StateFlow<MarketState> = mutable.asStateFlow()
    private var epoch=0L; private var catalogJob: Job?=null; private val jobs=mutableMapOf<String,Job>()
    fun initialize(): Job=scope.launch { safely { repository.initialize(); refresh(); mutable.update { it.copy(runtimeError=capability()) } } }
    fun query(text: String): Job { epoch++; catalogJob?.cancel(); val generation=epoch; mutable.update { it.copy(query=text.trim(),apps=emptyList(),cursor=null,error=null,loading=true) }; return load(generation,null).also { catalogJob=it } }
    fun loadNext(): Job? { val s=state.value; if(s.loading || s.cursor==null) return null; mutable.update { it.copy(loading=true,error=null) }; return load(epoch,s.cursor).also { catalogJob=it } }
    private fun load(generation: Long,cursor: String?)=scope.launch {
        try { val result=client.catalog(state.value.query,cursor); if(generation==epoch) mutable.update { it.copy(apps=(if(cursor==null) result.items else it.apps+result.items).distinctBy { a->a.id },cursor=result.nextCursor,loading=false) } }
        catch(e: CancellationException) { throw e } catch(e: Throwable) { if(generation==epoch) mutable.update { it.copy(loading=false,error=code(e)) } }
    }
    fun install(appId: String,versionId: String)=operation(appId,false,versionId)
    fun update(appId: String,versionId: String)=operation(appId,true,versionId)
    private fun operation(appId: String,update: Boolean,versionId: String): Job {
        jobs[appId]?.takeIf { it.isActive }?.let { return it }
        return scope.launch {
            op(appId,MarketOperation(OperationPhase.DOWNLOADING))
            try { repository.install(appId,versionId,update,{ bytes,total -> op(appId,MarketOperation(OperationPhase.DOWNLOADING,bytes,total)) },{ op(appId,MarketOperation(OperationPhase.COMMITTING)) }); refresh(); op(appId,MarketOperation(OperationPhase.COMPLETE)) }
            catch(e: CancellationException) { withContext(NonCancellable) { refresh(); op(appId,MarketOperation(if(state.value.installed.any { it.appId==appId && it.versionId==versionId }) OperationPhase.COMPLETE else OperationPhase.CANCELLED)) }; throw e }
            catch(e: Throwable) { op(appId,MarketOperation(OperationPhase.FAILED,error=code(e))) }
        }.also { jobs[appId]=it }
    }
    fun cancel(appId: String) { if(state.value.operations[appId]?.phase==OperationPhase.DOWNLOADING) jobs[appId]?.cancel() }
    fun uninstall(appId: String): Job {
        jobs[appId]?.takeIf { it.isActive }?.let { return it }
        return scope.launch { op(appId,MarketOperation(OperationPhase.UNINSTALLING)); try { repository.uninstall(appId); refresh(); op(appId,MarketOperation(OperationPhase.COMPLETE)) } catch(e: Throwable) { op(appId,MarketOperation(OperationPhase.FAILED,error=code(e))) } }.also { jobs[appId]=it }
    }
    fun checkUpdates(): Job=scope.launch { safely { val updates=client.updates(repository.installed()); mutable.update { it.copy(updates=updates,error=null) } } }
    suspend fun detail(appId: String)=client.detail(appId)
    suspend fun versions(appId: String,cursor: String?=null)=client.versions(appId,cursor)
    private suspend fun refresh() { val records=repository.installed(); mutable.update { it.copy(installed=records) } }
    private fun op(id: String,value: MarketOperation) { mutable.update { it.copy(operations=it.operations+(id to value)) } }
    private suspend fun safely(block: suspend ()->Unit) { try { block() } catch(e: CancellationException) { throw e } catch(e: Throwable) { mutable.update { it.copy(error=code(e)) } } }
    private fun code(e: Throwable)=(e as? MarketException)?.code ?: e.message ?: "UNKNOWN_ERROR"
}
class LocalAppMarket private constructor(val store: MarketStore,val runtime: MarketWebRuntime,val repository: MarketRepository) {
    companion object {
        /** Keep one instance in the application service graph; scope should use Main + SupervisorJob. */
        fun create(context: Context,apiRoot: String,scope: CoroutineScope): LocalAppMarket {
            val root=File(context.filesDir,"local-app-market"); val client=MarketClient(apiRoot)
            val runtime=MarketWebRuntime(context,root); val repository=MarketRepository(root,client,runtime); runtime.repository=repository
            return LocalAppMarket(MarketStore(client,repository,scope,runtime::capabilityError),runtime,repository)
        }
    }
}
