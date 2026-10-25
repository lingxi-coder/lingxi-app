export function originOf(value) {
  const url = new URL(value);
  if (!['http:','https:'].includes(url.protocol) || url.username || url.password) throw new Error('Only HTTP and HTTPS sites are supported');
  return url.origin;
}
export function permitsNavigation(policy,url) {
  try {
    const origin = originOf(url);
    const site = policy.sites.find(site => site.origin === origin);
    if (site?.browsing === 'deny') return false;
    return site?.browsing === 'allow' || policy.allowedOrigins.includes(origin) || policy.defaultBrowsing === 'allow';
  } catch { return false; }
}
export function permittedCdpMethod(method) {
  if (typeof method !== 'string' || method.length > 128 || !/^[A-Za-z]+\.[A-Za-z]+$/.test(method)) return false;
  const [domain] = method.split('.');
  if (!['Runtime','DOM','DOMSnapshot','Accessibility','CSS','Page','Performance','Log','Network','Emulation'].includes(domain)) return false;
  // Raw protocol access remains target-scoped and cannot remove the policy guards,
  // install persistent scripts, access profile-wide cookies, or create targets.
  return !['Page.navigate','Page.navigateToHistoryEntry','Page.setDownloadBehavior','Page.addScriptToEvaluateOnNewDocument','Page.removeScriptToEvaluateOnNewDocument','Page.createIsolatedWorld','Page.setDocumentContent','Network.getAllCookies','Network.getCookies','Network.setCookies','Network.setCookie','Network.deleteCookies','Network.clearBrowserCookies','Network.clearBrowserCache'].includes(method);
}
export function keyDescription(chord) {
  if (typeof chord !== 'string' || chord.length > 128) throw new Error('A key chord is required');
  const parts = chord.toLowerCase().split('+').map(value => value.trim()).filter(Boolean);
  let modifiers = 0; const keys = [];
  for (const part of parts) {
    if (['cmd','command','meta','super','win','windows'].includes(part)) modifiers |= 4;
    else if (['ctrl','control'].includes(part)) modifiers |= 2;
    else if (['alt','option'].includes(part)) modifiers |= 1;
    else if (part === 'shift') modifiers |= 8;
    else keys.push(part);
  }
  if (keys.length !== 1) throw new Error('Exactly one key is required');
  const key = keys[0];
  if (((modifiers & 6) && ['c','v','x','o','s','p','q'].includes(key)) || key === 'insert' || key === 'delete' && (modifiers & 8)) throw new Error('Clipboard, file and system shortcuts are unavailable through Browser key input');
  const navigation = ['left','right','up','down','home','end','pageup','pagedown','backspace','delete'];
  if (![0,8,1,2,4,9,10,12].includes(modifiers)
      || modifiers & 7 && !navigation.includes(key) && !(['a','z','y'].includes(key) && (modifiers & 6) && !(modifiers & 1))
      || modifiers & 2 && ['up','down','left','right'].includes(key)) throw new Error('Only browser-local text editing and navigation key chords are supported');
  const aliases = { return:'Enter',enter:'Enter',esc:'Escape',escape:'Escape',tab:'Tab',backspace:'Backspace',delete:'Delete',space:' ',left:'ArrowLeft',right:'ArrowRight',up:'ArrowUp',down:'ArrowDown',home:'Home',end:'End',pageup:'PageUp',pagedown:'PageDown' };
  return { key:aliases[key] ?? (key.length === 1 ? key : (() => { throw new Error('Unsupported browser key'); })()),modifiers };
}
