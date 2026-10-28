import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createRemoteLocalAppsDelegate } from '../src/main/remoteControl/localAppsBindings.js';
import type { RemoteViewer } from '../src/main/remoteControl/gateway.js';

const viewer: RemoteViewer = { account_uid: 'account', host_id: 'host', device_id: 'device', conn_id: 'conn', device_name: 'phone', platform: 'ios' };
test('in-flight access is denied after account or device changes, and only the new ticket is revoked', async () => {
  let account = 'account', approved = true, generation = 0;
  let resolve!: (value: never) => void;
  let revoked: unknown;
  const wait = () => new Promise<never>(r => { resolve = r; });
  const host = { remoteList: wait, remoteOpen: wait, remoteRevokeOwner() { throw new Error('must preserve existing sessions'); }, remoteRevokeTicket(url: string) { revoked = url; }, remoteRevokeAll() {} };
  const delegate = createRemoteLocalAppsDelegate({ host: () => host, accountUid: () => account, hostId: () => 'host', active: () => true, generation: () => generation, approvedDevice: () => approved });
  const list = delegate.list(viewer); account = 'other'; resolve([] as never);
  await assert.rejects(list, /no longer approved/);
  account = 'account';
  const open = delegate.open(viewer, 'a' + '0'.repeat(32)); approved = false; resolve({url:'https://new.example/__lingxi_remote_bootstrap#ticket'} as never);
  await assert.rejects(open, /no longer approved/);
  assert.equal(revoked, 'https://new.example/__lingxi_remote_bootstrap#ticket');
  approved = true; revoked = undefined;
  const reopened = delegate.open(viewer, 'a' + '0'.repeat(32)); generation++; resolve({url:'https://next.example/__lingxi_remote_bootstrap#ticket'} as never);
  await assert.rejects(reopened, /access changed/);
  assert.equal(revoked, 'https://next.example/__lingxi_remote_bootstrap#ticket');
  delegate.viewerClosed(viewer);
});

test('another owners revocation does not invalidate an approved pending open', async () => {
  const versions=new Map<string,number>();let resolve!:(value:any)=>void;const revoked:unknown[]=[];
  const host={remoteList:async()=>[],remoteOpen:()=>new Promise<any>(done=>{resolve=done;}),remoteRevokeOwner:(owner:unknown)=>{revoked.push(owner);},remoteRevokeTicket:(url:string)=>{revoked.push(url);},remoteRevokeAll(){}};
  const delegate=createRemoteLocalAppsDelegate({host:()=>host,accountUid:()=>viewer.account_uid,hostId:()=>viewer.host_id,active:()=>true,generation:v=>versions.get(v.device_id)??0,approvedDevice:()=>true});
  const pending=delegate.open(viewer,'a'+'0'.repeat(32));versions.set('other-device',1);const result={app_id:'a'+'0'.repeat(32)};resolve(result);
  assert.deepEqual(await pending,result);assert.deepEqual(revoked,[]);
});

test('transport loss after open revokes only the new ticket', async () => {
  let active=true, resolve!:(value:any)=>void;const revoked:string[]=[];
  const host={remoteList:async()=>[],remoteOpen:()=>new Promise<any>(done=>{resolve=done;}),remoteRevokeOwner:()=>{throw Error('existing sessions revoked');},remoteRevokeTicket:(url:string)=>{revoked.push(url);},remoteRevokeAll(){}};
  const delegate=createRemoteLocalAppsDelegate({host:()=>host,accountUid:()=>viewer.account_uid,hostId:()=>viewer.host_id,active:()=>active,generation:()=>0,approvedDevice:()=>true});
  const pending=delegate.open(viewer,'a'+'0'.repeat(32));active=false;resolve({url:'https://new.example/__lingxi_remote_bootstrap#ticket'});
  await assert.rejects(pending,/no longer approved/);assert.deepEqual(revoked,['https://new.example/__lingxi_remote_bootstrap#ticket']);
});
