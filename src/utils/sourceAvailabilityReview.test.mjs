import test from 'node:test';
import assert from 'node:assert/strict';
import { SourceAvailabilityReview } from './sourceAvailabilityReview.ts';
const deferred = () => { let resolve, reject; const promise = new Promise((a,b) => { resolve=a; reject=b; }); return {promise,resolve,reject}; };
const observation = (sources = [
  {source_id:'A',label:'A',status:'unavailable',reason:'not_found'},
  {source_id:'B',label:'B',status:'available',reason:null},
]) => ({admission:'not_product_admission',sources,observed_conflict:false});

test('mount and blocked/context changes perform no automatic read', async () => {
  let calls=0;const review=new SourceAvailabilityReview(async()=>{calls++;return observation();});
  await review.check();review.setContext({});review.setContext(null);review.setContext({});
  assert.equal(calls,0);assert.equal(review.getSnapshot().phase,'idle');
});
test('one explicit check retains A unavailable and B available without dropping or granting access', async () => {
  const result=observation();let calls=0;const review=new SourceAvailabilityReview(async()=>{calls++;return result;});
  review.setContext({});await review.check();
  assert.equal(calls,1);assert.equal(review.getSnapshot().phase,'observed');assert.deepEqual(review.getSnapshot().observation,result);
  assert.deepEqual(Object.keys(review.getSnapshot()).sort(),['context','observation','pending','phase']);
});
test('rapid duplicate checks share no second IPC while the first is pending', async () => {
  const work=deferred();let calls=0;const review=new SourceAvailabilityReview(()=>{calls++;return work.promise;});
  review.setContext({});const first=review.check();await review.check();await review.check();
  assert.equal(calls,1);assert.equal(review.getSnapshot().pending,true);work.resolve(observation());await first;
  assert.equal(review.getSnapshot().pending,false);
});
test('recheck clears earlier rows immediately and failure never retains an available label', async () => {
  const work=deferred();let calls=0;const review=new SourceAvailabilityReview(()=>++calls===1?Promise.resolve(observation()):work.promise);
  review.setContext({});await review.check();const again=review.check();
  assert.equal(review.getSnapshot().observation,null);assert.equal(review.getSnapshot().phase,'loading');
  work.reject(new Error('SOURCE_AVAILABILITY_BUSY'));await again;
  assert.equal(calls,2);assert.equal(review.getSnapshot().phase,'failed');assert.equal(review.getSnapshot().observation,null);
});
test('old reply after catalog/config context switch is discarded and needs a new manual check', async () => {
  const work=deferred();let calls=0;const review=new SourceAvailabilityReview(()=>{calls++;return work.promise;});
  review.setContext({});const first=review.check();const next={};review.setContext(next);await review.check();
  assert.equal(calls,1);work.resolve(observation());await first;
  assert.equal(review.getSnapshot().context,next);assert.equal(review.getSnapshot().phase,'idle');assert.equal(review.getSnapshot().observation,null);
});
test('block then same context reopening cannot resurrect the earlier reply', async () => {
  const work=deferred();const context={};const review=new SourceAvailabilityReview(()=>work.promise);
  review.setContext(context);const first=review.check();review.setContext(null);review.setContext(context);
  work.resolve(observation());await first;assert.equal(review.getSnapshot().phase,'idle');assert.equal(review.getSnapshot().observation,null);
});
test('unmount invalidation keeps pending IPC accounted without automatic cancellation or retry', async () => {
  const work=deferred();let calls=0,notifications=0;const review=new SourceAvailabilityReview(()=>{calls++;return work.promise;});
  const unsubscribe=review.subscribe(()=>notifications++);review.setContext({});const first=review.check();review.setContext(null);unsubscribe();const count=notifications;
  assert.equal(review.getSnapshot().pending,true);await review.check();assert.equal(calls,1);
  work.resolve(observation());await first;assert.equal(notifications,count);assert.equal(review.getSnapshot().observation,null);assert.equal(review.getSnapshot().pending,false);
});
test('failed checks remain manual even when a later context becomes ready', async () => {
  let calls=0;const review=new SourceAvailabilityReview(async()=>{calls++;throw new Error('SOURCE_AVAILABILITY_UNCONFIRMED');});
  review.setContext({});await review.check();review.setContext(null);review.setContext({});await Promise.resolve();
  assert.equal(calls,1);assert.equal(review.getSnapshot().phase,'idle');await review.check();assert.equal(calls,2);assert.equal(review.getSnapshot().phase,'failed');
});
test('identity conflict remains an observation and empty success is distinct from failure', async () => {
  const conflict={...observation([{source_id:'A',label:'A',status:'identity_conflict',reason:'origin_mismatch'}]),observed_conflict:true};
  let count=0;const review=new SourceAvailabilityReview(async()=>count++===0?conflict:observation([]));review.setContext({});await review.check();
  assert.deepEqual(review.getSnapshot().observation,conflict);await review.check();assert.equal(review.getSnapshot().phase,'observed');assert.deepEqual(review.getSnapshot().observation.sources,[]);
});
