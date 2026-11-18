import {useEffect,useState} from 'react';
import type {InboxNoticeTarget} from './contracts.js';
import {executionCommand,type TeamTaskView} from '../teams/execution.js';
import {serviceCommand,type HostedServicesApi} from '../teams/hosted-service.js';
import type {MessageApi} from '../teams/messages.js';

interface Scope {kind:'team'|'service';sourceId:string;generation:number;conversationId:string}
type SourceApi=Pick<MessageApi,'messages'>|Pick<HostedServicesApi,'serviceMessages'>;
interface Preview {text?:string;recalled?:boolean;task?:TeamTaskView}
export async function readSourceNotice(api:SourceApi,scope:Scope,target:InboxNoticeTarget):Promise<Preview>{
  const key=[scope.kind,scope.sourceId,scope.conversationId].join(':');
  if(target.key && target.key!==key || target.generation!==undefined && target.generation!==scope.generation)throw Error('STALE_MEMBERSHIP');
  if(target.taskId){
    const payload={taskId:target.taskId};
    const task='messages' in api
      ?await api.messages<TeamTaskView>(executionCommand({teamId:scope.sourceId,generation:scope.generation},'getTask',payload))
      :await api.serviceMessages<TeamTaskView>(serviceCommand({serviceId:scope.sourceId,generation:scope.generation},'getTask',payload));
    if(task.taskId!==target.taskId || task.conversationId!==scope.conversationId)throw Error('TARGET_UNAVAILABLE');
    return {task};
  }
  if(!Number.isSafeInteger(target.through)||target.through<1)throw Error('TARGET_UNAVAILABLE');
  type Page={messages:Array<{id:string;seq:number;text:string;recalled?:boolean}>};
  const command={action:'history' as const,conversationId:scope.conversationId,generation:scope.generation,before:target.through+1};
  const page='messages' in api?await api.messages<Page>({...command,teamId:scope.sourceId}):await api.serviceMessages<Page>({...command,serviceId:scope.sourceId});
  const found=page.messages.find(row=>row.seq===target.through && (!target.messageId || row.id===target.messageId));
  if(!found)throw Error('TARGET_UNAVAILABLE');
  return {text:found.text,recalled:found.recalled};
}
/** A read-only source preview. The normal transcript and its exposure observer mount only after an explicit exit. */
export function SourceNoticePreview({api,kind,sourceId,generation,conversationId,target,locale,onReturn}:Scope & {
  api:SourceApi;target:InboxNoticeTarget;locale:string;onReturn():void;
}){
  const cn=locale.startsWith('zh');
  const [value,setValue]=useState<Preview|null>(),[failed,setFailed]=useState(false);
  useEffect(()=>{
    let live=true;setValue(undefined);setFailed(false);
    void readSourceNotice(api,{kind,sourceId,generation,conversationId},target).then(v=>{if(live)setValue(v);}).catch(()=>{if(live)setFailed(true);});
    return ()=>{live=false;};
  },[api,kind,sourceId,generation,conversationId,target.through,target.messageId,target.taskId,target.key,target.generation]);
  const task=value?.task;
  return <section className="tw-scope tw-conv" aria-label={cn?'通知预览':'Notification preview'}>
    <header className="tw-conv-head"><h2>{task?(cn?'任务通知':'Task notification'):(cn?'消息通知':'Message notification')}</h2></header>
    {failed?<p role="status">{cn?'这条通知的内容已不可用。':'The content of this notification is no longer available.'}</p>
    :task?<p>{task.wait?.kind==='tool_approval'?(cn?'待批准':'Approval needed'):task.wait?.kind==='human_input'?(cn?'待输入':'Input needed'):task.state}</p>
    :value?<p>{value.recalled?(cn?'此消息已撤回。':'This message was recalled.'):value.text}</p>:null}
    <button type="button" onClick={onReturn}>{cn?'返回会话':'Return to conversation'}</button>
  </section>;
}
