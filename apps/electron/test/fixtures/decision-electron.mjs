import assert from 'node:assert/strict';
import { app, BrowserWindow } from 'electron';
import { writeFileSync } from 'node:fs';
const url=process.argv.find(value=>value.startsWith('http://'));
if(!url||!process.env.LINGXI_TEST_USER_DATA)throw new Error('isolated fixture required');
app.setPath('userData',process.env.LINGXI_TEST_USER_DATA);
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));
async function main(){
 await app.whenReady();const win=new BrowserWindow({show:false,width:1000,height:1100,webPreferences:{sandbox:true,backgroundThrottling:false}});
 const run=code=>win.webContents.executeJavaScript(code);
 const wait=async expression=>{for(let n=0;n<150;n++){if(await run(expression))return;await delay(20);}throw new Error(`Timed out: ${expression}`);};
 const button=text=>run(`Array.from(document.querySelectorAll('button')).find(el=>el.textContent===${JSON.stringify(text)})?.click()`);
 const fill=(label,value,tag='input',index=0)=>run(`(()=>{const labels=Array.from(document.querySelectorAll('label')).filter(el=>el.textContent.startsWith(${JSON.stringify(label)}));const el=labels[${index}]?.querySelector(${JSON.stringify(tag)});if(!el)throw new Error('Missing label '+${JSON.stringify(label)});const prototype=el.tagName==='TEXTAREA'?HTMLTextAreaElement.prototype:el.tagName==='SELECT'?HTMLSelectElement.prototype:HTMLInputElement.prototype;Object.getOwnPropertyDescriptor(prototype,'value').set.call(el,${JSON.stringify(value)});el.dispatchEvent(new Event(el.tagName==='SELECT'?'change':'input',{bubbles:true}));})()`);
 try{
  await win.loadURL(url);await wait(`Array.from(document.querySelectorAll('button')).find(el=>el.textContent==='Save')?.disabled===false`);
  await fill('Service','custom','select');await fill('Model','local-model');await fill('API Key','private-fixture-key');
  await run('window.decisionFixture.hold()');await button('Save');await wait(`Array.from(document.querySelectorAll('button')).find(el=>el.textContent==='Save')?.disabled===true`);
  assert.equal(await run('window.decisionFixture.calls.length'),0);await run('window.decisionFixture.release()');await wait(`document.body.textContent.includes('custom · local-model')`);
  await fill('Evidence','Please refund this charge.','textarea');await fill('Instructions','Is a refund requested?','textarea');await button('Run test');await wait(`document.querySelector('pre')?.textContent.includes('0.9')`);
  await button('Add question');await fill('Type','choice','select',1);await fill('Instructions','Choose a team','textarea',1);await fill('One option per line','billing=Billing\nother=Other','textarea');
  await button('Add question');await fill('Type','score','select',2);await fill('Instructions','Rate urgency','textarea',2);await fill('One level per line','Low\nHigh','textarea');
  await button('Run test');await wait(`window.decisionFixture.calls.length===2 && document.querySelector('pre')?.textContent.includes('"score"')`);
  assert.equal(await run('Object.keys(window.decisionFixture.calls[1].questions).length'),3);
  await run('window.decisionFixture.hold()');await button('Run test');await wait(`Array.from(document.querySelectorAll('button')).some(el=>el.textContent==='Cancel')`);await button('Cancel');await wait(`document.querySelector('[role=alert]')?.textContent.includes('cancelled')`);
  await run('window.decisionFixture.external()');await wait(`document.body.textContent.includes('Settings changed in another window')`);await button('Save');await wait(`document.querySelector('[role=alert]')?.textContent.includes('changed')`);await button('Reload');await wait(`document.querySelector('input[readonly]')?.value==='https://api.typesafe.ai/v1/systemone'`);
  await run(`document.querySelector('main').scrollTop=0`);await delay(50);
  if(process.env.LINGXI_DECISION_SCREENSHOT_DIR){writeFileSync(`${process.env.LINGXI_DECISION_SCREENSHOT_DIR}/decision-light.png`,(await win.webContents.capturePage()).toPNG());await run('window.decisionFixture.setDark(true)');await delay(50);writeFileSync(`${process.env.LINGXI_DECISION_SCREENSHOT_DIR}/decision-dark.png`,(await win.webContents.capturePage()).toPNG());}
  win.setSize(640,1000);await delay(50);assert.equal(await run('document.documentElement.scrollWidth<=window.innerWidth'),true);
  console.log(JSON.stringify({passed:true,calls:await run('window.decisionFixture.calls.length')}));
 }finally{win.destroy();}
}
main().then(()=>app.exit(0)).catch(error=>{console.error(error);app.exit(1);});
