const copyButton = document.querySelector("[data-copy-target]");

copyButton?.addEventListener("click", async () => {
  const target = document.getElementById(copyButton.dataset.copyTarget);
  if (!target) return;

  try {
    await navigator.clipboard.writeText(target.innerText);
    copyButton.textContent = "Copied";
    window.setTimeout(() => {
      copyButton.textContent = "Copy";
    }, 1800);
  } catch {
    copyButton.textContent = "Select code";
    const selection = window.getSelection();
    const range = document.createRange();
    range.selectNodeContents(target);
    selection.removeAllRanges();
    selection.addRange(range);
  }
});

const year = document.querySelector("[data-current-year]");
if (year) year.textContent = new Date().getFullYear();
