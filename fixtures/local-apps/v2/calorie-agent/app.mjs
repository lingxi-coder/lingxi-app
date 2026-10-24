import { MealAgentClient, terminalStates, revisionStorage } from './agent-client.mjs';
import { nutritionSchema, recognitionPrompt, checkedResult, calorieRange, mealRecord } from './nutrition.mjs';

const $ = (id) => document.getElementById(id);
let agent = window.localApp?.agent;
let taskStorage;
let hostError = '';
if (agent) {
  try { taskStorage = revisionStorage(sessionStorage, (await window.localApp.info()).revision); }
  catch (failure) { agent = null; hostError = `无法读取应用运行版本：${failure.message}。请重新打开应用。`; }
}
const historyKey = 'meal-notes.records.v1';
let image = null;
let stream = null;
let result = null;
let busy = false;
let completedRunId = null;
let resultRunId = null;
let cameraGeneration = 0;

function error(message) {
  $('error').textContent = message;
  $('error').hidden = !message;
}

function setBusy(value) {
  busy = value;
  $('analyze').disabled = busy || !agent || (!image && !client?.pending);
  $('analyze').textContent = client?.pending ? '重试同一请求' : '识别这一餐';
  $('cancel').hidden = !busy || Boolean(client?.pending) || !client?.runId;
  $('pending-review').hidden = busy || !client?.pending;
  if (!client?.pending) $('discard-confirm').checked = false;
  $('discard-pending').disabled = busy || !$('discard-confirm').checked;
  $('camera-open').disabled = busy || Boolean(client?.pending);
  $('photo-file').disabled = busy || Boolean(client?.pending);
  $('photo-choose').disabled = busy || Boolean(client?.pending);
  $('notes').disabled = busy || Boolean(client?.pending);
}

function stopCamera() {
  cameraGeneration += 1;
  stream?.getTracks().forEach((track) => track.stop());
  stream = null;
  $('camera').srcObject = null;
  $('camera').hidden = true;
  $('camera-capture').hidden = true;
  $('camera-close').hidden = true;
  $('photo').hidden = !image;
  $('photo-placeholder').hidden = Boolean(image);
}

function setPhoto(source) {
  const width = source.videoWidth || source.width;
  const height = source.videoHeight || source.height;
  if (!width || !height) throw new Error('照片无法读取，请换一张照片。');
  const scale = Math.min(1, 1536 / Math.max(width, height));
  const canvas = document.createElement('canvas');
  canvas.width = Math.max(1, Math.round(width * scale));
  canvas.height = Math.max(1, Math.round(height * scale));
  const context = canvas.getContext('2d');
  context.fillStyle = '#fff';
  context.fillRect(0, 0, canvas.width, canvas.height);
  context.drawImage(source, 0, 0, canvas.width, canvas.height);
  const dataUrl = canvas.toDataURL('image/jpeg', .86);
  client?.forgetCompleted();
  result = null;
  completedRunId = null;
  resultRunId = null;
  $('result').hidden = true;
  image = { media_type: 'image/jpeg', base64: dataUrl.split(',')[1] };
  $('photo').src = dataUrl;
  stopCamera();
  $('status').textContent = '照片已准备好，确认后点击识别。';
  error('');
  setBusy(false);
}

async function nativePhoto(method) {
  const value = await method();
  if (!value) return;
  if (!['image/jpeg', 'image/png', 'image/webp'].includes(value.mimeType) || typeof value.base64 !== 'string') {
    throw new Error('请选择 JPEG、PNG 或 WebP 照片。');
  }
  const photo = new Image();
  photo.src = `data:${value.mimeType};base64,${value.base64}`;
  await photo.decode();
  setPhoto(photo);
}
$('camera-open').addEventListener('click', async () => {
  try {
    if (window.localApp?.camera?.capture) {
      await nativePhoto(() => window.localApp.camera.capture());
      return;
    }
    stopCamera();
    const generation = cameraGeneration;
    const requestedStream = await navigator.mediaDevices.getUserMedia({ video: { facingMode: 'environment' }, audio: false });
    if (generation !== cameraGeneration || busy) { requestedStream.getTracks().forEach((track) => track.stop()); return; }
    stream = requestedStream;
    $('camera').srcObject = stream;
    await $('camera').play();
    $('camera').hidden = false;
    $('photo').hidden = true;
    $('photo-placeholder').hidden = true;
    $('camera-capture').hidden = false;
    $('camera-close').hidden = false;
    error('');
  } catch (failure) {
    stopCamera();
    error(`无法打开相机：${failure.message}。也可以选择照片。`);
  }
});
$('camera-close').addEventListener('click', stopCamera);
$('photo-choose').addEventListener('click', async () => {
  try {
    if (window.localApp?.photos?.pick) await nativePhoto(() => window.localApp.photos.pick());
    else $('photo-file').click();
  } catch (failure) { error(failure.message); }
});
$('camera-capture').addEventListener('click', () => {
  try { setPhoto($('camera')); } catch (failure) { error(failure.message); }
});
$('photo-file').addEventListener('change', async (event) => {
  const file = event.target.files?.[0];
  if (!file) return;
  try {
    if (!['image/jpeg', 'image/png', 'image/webp'].includes(file.type) || file.size > 20 * 1024 * 1024) {
      throw new Error('请选择 20 MB 以内的 JPEG、PNG 或 WebP 照片。');
    }
    const bitmap = await createImageBitmap(file);
    try { setPhoto(bitmap); } finally { bitmap.close(); }
  } catch (failure) { error(failure.message); }
  event.target.value = '';
});

function edits() {
  return [...$('food-items').querySelectorAll('.food')].map((row) => ({
    name: row.querySelector('[data-field="name"]').value,
    grams: row.querySelector('[data-field="grams"]').value,
  }));
}

function updateTotal() {
  try {
    const values = edits();
    const range = calorieRange(result.items.map((item, index) => ({ ...item, grams: values[index].grams })));
    $('total').textContent = `${range.min}–${range.max}`;
    $('save').disabled = result.items.length === 0;
  } catch {
    $('total').textContent = '请检查份量';
    $('save').disabled = true;
  }
}

function listText(id, heading, entries) {
  $(id).replaceChildren();
  if (!entries.length) return;
  const title = document.createElement('h3');
  title.textContent = heading;
  const list = document.createElement('ul');
  for (const entry of entries) {
    const item = document.createElement('li');
    item.textContent = entry;
    list.append(item);
  }
  $(id).append(title, list);
}

function renderResult(value) {
  result = checkedResult(value);
  $('food-items').replaceChildren();
  result.items.forEach((item, index) => {
    const row = document.createElement('div');
    row.className = 'food';
    for (const [field, labelText] of [['name', '食物'], ['grams', '份量（克）']]) {
      const label = document.createElement('label');
      label.textContent = labelText;
      const input = document.createElement('input');
      input.dataset.field = field;
      input.id = `food-${index}-${field}`;
      input.value = item[field];
      if (field === 'grams') { input.type = 'number'; input.min = '1'; input.max = '10000'; input.step = 'any'; }
      else input.maxLength = 120;
      input.addEventListener('input', updateTotal);
      label.append(input);
      row.append(label);
    }
    const density = document.createElement('p');
    density.className = 'fine';
    density.textContent = `模型估算：${item.kcalPer100gMin}–${item.kcalPer100gMax} kcal / 100 克`;
    row.append(density);
    $('food-items').append(row);
  });
  listText('assumptions', '估算基于这些假设', result.assumptions);
  listText('questions', '这些信息可以让估算更准确', result.questions);
  $('result').hidden = false;
  updateTotal();
}

const phases = { starting: '正在连接模型…', thinking: '正在识别餐食…', tool: '正在处理结果…',
  waiting_for_permission: '请在 LingXi 宿主窗口中处理工具授权。', waiting_for_answer: '请在 LingXi 宿主窗口中回答待确认问题。' };
const client = agent ? new MealAgentClient(agent, taskStorage, (event) => {
  if (event.type === 'progress') {
    $('status').textContent = phases[event.phase] || '正在处理…';
    return;
  }
  const run = event.run;
  setBusy(!terminalStates.has(run.state));
  if (run.state === 'succeeded') {
    completedRunId = run.runId;
    if (resultRunId !== run.runId) {
      try { renderResult(run.result?.output); resultRunId = run.runId; }
      catch (failure) { error(failure.message); $('status').textContent = '结果未通过应用校验，请重试。'; return; }
    }
    $('status').textContent = '识别完成。请确认食物与份量，再保存记录。';
  } else if (terminalStates.has(run.state)) {
    $('status').textContent = run.state === 'cancelled' ? '识别已取消。' : run.state === 'interrupted' ? '任务已中断，请手动重试。' : '识别失败。';
    error(run.error ? `${run.error.message} (${run.error.code})` : '');
  } else $('status').textContent = run.state === 'queued' ? '请求已提交…' : '正在识别餐食…';
}) : null;

$('analyze').addEventListener('click', async () => {
  if (busy || !client || (!image && !client.pending)) return;
  error('');
  stopCamera();
  $('result').hidden = true;
  result = null;
  completedRunId = null;
  resultRunId = null;
  setBusy(true);
  try {
    const request = client.pending ?? { requestId: crypto.randomUUID(), prompt: recognitionPrompt($('notes').value),
      images: [image], outputSchema: nutritionSchema, tools: [], maxTurns: 4, timeoutMs: 120000 };
    await client.submit(request);
  } catch (failure) { error(`${failure.message}。可重试同一请求；请先在 LingXi 的 Apps 中检查本应用的 Agent 授权和模型。`); setBusy(false); }
});
$('cancel').addEventListener('click', async () => {
  try { await client.cancel(); } catch (failure) { error(`取消未确认：${failure.message}`); }
});
$('discard-confirm').addEventListener('change', () => {
  $('discard-pending').disabled = busy || !$('discard-confirm').checked;
});
$('discard-pending').addEventListener('click', () => {
  try {
    if (!client.discardPending($('discard-confirm').checked)) return;
    stopCamera();
    image = null;
    result = null;
    completedRunId = null;
    resultRunId = null;
    $('photo').removeAttribute('src');
    $('photo').hidden = true;
    $('photo-placeholder').hidden = false;
    $('result').hidden = true;
    $('status').textContent = '已放弃原请求的恢复记录。请选择新的照片；识别只会在你再次点击后开始。';
    error('');
    setBusy(false);
  } catch (failure) { error(failure.message); }
});

function readHistory() {
  const saved = JSON.parse(localStorage.getItem(historyKey) || '[]');
  if (!Array.isArray(saved)) throw new Error('已保存记录格式无效。请保留应用数据并联系维护者。');
  return saved;
}

function renderHistory() {
  const records = readHistory();
  $('history').replaceChildren();
  if (!records.length) { $('history').textContent = '还没有记录。确认第一餐后，它会出现在这里。'; return; }
  for (const record of records) {
    const article = document.createElement('article');
    article.className = 'record';
    const heading = document.createElement('strong');
    heading.textContent = `${record.range.min}–${record.range.max} kcal · 模型估算`;
    const detail = document.createElement('p');
    detail.textContent = record.items.map((item) => `${item.name} ${item.grams} 克`).join('、');
    const date = document.createElement('p');
    date.className = 'fine';
    date.textContent = new Date(record.savedAt).toLocaleString();
    article.append(heading, detail, date);
    $('history').append(article);
  }
}
$('save').addEventListener('click', () => {
  try {
    const record = mealRecord(result, edits(), completedRunId);
    const records = readHistory();
    localStorage.setItem(historyKey, JSON.stringify([record, ...records.filter((item) => item.id !== record.id)]));
    renderHistory();
    $('status').textContent = '已保存。照片未保存在餐食记录中。';
    error('');
  } catch (failure) { error(`未保存：${failure.message}`); }
});
window.addEventListener('pagehide', () => { stopCamera(); client?.dispose(); });
try { renderHistory(); } catch (failure) { error(failure.message); }
if (!client) {
  $('status').textContent = hostError || '请从支持 Agent 的 LingXi Desktop Apps 中打开。此示例不在手机或普通浏览器中调用 Agent。';
} else {
  setBusy(true);
  client.resume().then((runId) => {
    if (!runId) {
      setBusy(false);
      if (client.pending) $('status').textContent = '保留了一条未确认请求，请选择重试原请求或放弃后重新选图。';
    }
  }).catch((failure) => { error(`恢复任务失败：${failure.message}`); setBusy(false); });
}
