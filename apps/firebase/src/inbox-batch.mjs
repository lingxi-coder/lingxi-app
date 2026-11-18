import { FieldPath } from 'firebase-admin/firestore';
import { DAY, deny, digest, millis, searchTitle } from './inbox-model.mjs';

const view=(id,v)=>({jobId:id,state:v.state,processed:v.processed});
const jobRef=(db,uid,id)=>db.collection('users').doc(uid).collection('inboxReadBatches').doc(id);
export async function startReadBatch(store,uid,p) {
  const doc=jobRef(store.db,uid,p.operationId),now=store.clock();
  const scope={archived:p.archived??null,filter:p.filter??'all',category:p.category??null,prefix:searchTitle(p.searchPrefix??'')};
  const signature=digest(scope);
  return store.db.runTransaction(async tx=>{
    const current=(await tx.get(doc)).data();
    if(current) {
      if(current.signature!==signature) deny(409,'OPERATION_CONFLICT');
      return view(doc.id,current);
    }
    if(!p.requestedAt || p.requestedAt<now-300000 || p.requestedAt>now+60000) deny(409,'BATCH_REQUEST_EXPIRED');
    const v={state:'queued',processed:0,signature,scope,after:null,expiresAt:new Date(now+30*DAY)};
    tx.create(doc,v);
    return view(doc.id,v);
  });
}
export async function readBatchStatus(store,uid,id){
  const doc=await jobRef(store.db,uid,id).get();
  if(!doc.exists || millis(doc.data().expiresAt)<=store.clock()) deny(404,'INBOX_JOB_MISSING');
  return view(id,doc.data());
}
export async function runReadBatch(store,uid,id,enqueue) {
  const job=jobRef(store.db,uid,id), snap=await job.get(), v=snap.data();
  if(!v || !['queued','running'].includes(v.state)) return;
  const boundary=snap.createTime;
  if(store.clock()-boundary.toMillis()>55*60000) {
    await job.update({state:'expired'});return;
  }
  // Firestore's historical snapshot freezes membership generations, action
  // revisions and cursors even as earlier pages change under new live traffic.
  const page=await store.db.runTransaction(async tx=>{
    let q=job.parent.parent.collection('inbox').where('isUnread','==',true).orderBy(FieldPath.documentId());
    if(v.after) q=q.startAfter(v.after);
    return tx.get(q.limit(50));
  },{readOnly:true,readTime:boundary});
  const targets=page.docs.map(d=>d.data()).filter(r=>(v.scope.archived===null || r.archived===v.scope.archived) && (!v.scope.category || r.category===v.scope.category) &&
    (v.scope.filter!=='attention' || r.needsAttention) && (!v.scope.prefix || r.titleSort.startsWith(v.scope.prefix)));
  // Independent recipient rows, bounded parallelism. Replay is harmless after
  // any partial failure because the frozen read and manual version never move.
  let index=0;
  await Promise.all(Array.from({length:Math.min(6,targets.length)},async()=>{
    while(index<targets.length) {
      const row=targets[index++];
      try { await store.mutate(uid,{action:'markRead',key:row.key,generation:row.ref.generation,through:row.sourceCursor,
        manualUnreadRevision:row.manualUnreadRevision,observedPendingActionRevision:row.pendingActionRevision}); }
      catch(e){if(![403,404,409,410].includes(e.status))throw e;}
    }
  }));
  const more=page.size===50;
  await store.db.runTransaction(async tx=>{
    const cur=(await tx.get(job)).data();
    if(!cur || cur.after!==v.after || !['queued','running'].includes(cur.state)) return;
    tx.update(job,{state:more?'running':'complete',after:page.docs.at(-1)?.id??cur.after,processed:cur.processed+targets.length});
  });
  if(more) await enqueue({type:'readBatch',uid,id});
}
