const output = document.querySelector('#status');
function render() { output.textContent = `Saved count: ${Number(localStorage.getItem('acceptance-count') || 0)}`; }
document.querySelector('#increment').addEventListener('click', () => {
  try { localStorage.setItem('acceptance-count', String(Number(localStorage.getItem('acceptance-count') || 0) + 1)); render(); }
  catch (error) { output.textContent = `Save failed: ${error.message}`; }
});
render();
