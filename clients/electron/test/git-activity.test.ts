import { test } from 'node:test';
import assert from 'node:assert/strict';
import { GitActivityTracker } from '../src/main/git-activity';
test('Git worktree guard tracks background workers independently of the parent turn', () => {
  const tracker = new GitActivityTracker();
  tracker.accept({type:'session_agent_updated',session_id:'s',agent:{agent_id:'a',name:'Agent',agent_type:'task',status:'running'}});
  assert.equal(tracker.active,true);
  tracker.accept({type:'turn_ended'} as any);
  assert.equal(tracker.active,true);
  tracker.accept({type:'session_agent_updated',session_id:'s',agent:{agent_id:'a',name:'Agent',agent_type:'task',status:'completed'}});
  assert.equal(tracker.active,false);
  tracker.accept({type:'coordinator_status',active_workers:1});
  assert.equal(tracker.active,true);
  tracker.accept({type:'coordinator_status',active_workers:0});
  assert.equal(tracker.active,false);
  tracker.accept({type:'session_agent_list',session_id:'s',agents:[{agent_id:'b',name:'B',agent_type:'task',status:'pending'}]});
  assert.equal(tracker.active,true);
  tracker.reset(); assert.equal(tracker.active,false);
});
