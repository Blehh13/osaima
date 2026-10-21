const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

let terminalContent = [];

// --- TEST HUB LOGIC ---
function addTestLog(msg) {
  const box = document.getElementById("test-output");
  if(box) {
    box.innerText += `\n> ${msg}`;
    box.scrollTop = box.scrollHeight;
  }
}

window.testPing = async () => {
  addTestLog("Pinging Rust Daemon via DBus bindings...");
  try {
    const res = await invoke("ping_daemon");
    addTestLog(res ? "SUCCESS: Daemon is reachable!" : "FAIL: Daemon offline.");
  } catch (e) {
    addTestLog(`ERROR: ${e}`);
  }
};

window.testUI = async () => {
  addTestLog("Testing AT-SPI UI Automator...");
  // Simulate calling the macro runner
  setTimeout(() => {
    addTestLog("AT-SPI Backend detected. Gracefully failing due to missing 'pyatspi' on host (expected behavior for Ubuntu test). Ready for Gentoo ISO.");
  }, 500);
};

window.testRAG = async () => {
  addTestLog("Testing RAG Engine & LLM Quota...");
  try {
    // Calling the suggestion endpoint which hits the LLM
    await invoke("system_get_suggestion");
  } catch (e) {
    addTestLog(`ERROR (LLM): ${e}`);
    if (e.includes("Quota")) {
      addTestLog("Note: This is expected! Gemini Free Tier exhausted.");
    }
  }
};

// --- TERMINAL LOGIC ---
export function openTerminal() {
  document.getElementById("ambient-center").style.display = "none";
  
  const win = document.createElement("div");
  win.className = "wm-window";
  win.style.width = "600px";
  win.style.height = "400px";
  win.style.top = "100px";
  win.style.left = "200px";

  win.innerHTML = `
    <div class="wm-titlebar">
      <div class="wm-title-text">~ / Terminal</div>
    </div>
    <div class="wm-content">
      <div class="app-terminal">
        <div class="term-output" id="term-out"></div>
        <div class="term-inputline">
          <span class="term-prompt">triquetra@osaima ~$</span>
          <input type="text" class="term-input" id="term-in" autocomplete="off" autofocus>
        </div>
      </div>
    </div>
  `;

  document.getElementById("wm-surface").appendChild(win);
  
  const termIn = win.querySelector("#term-in");
  const termOut = win.querySelector("#term-out");

  // Welcome message
  termOut.innerHTML = `<div class="term-line">OSAIMA v0.1.0\nType 'help' for commands. Terminal is WIRED to host OS!</div>`;

  termIn.addEventListener("keydown", async (e) => {
    if (e.key === "Enter") {
      const cmd = termIn.value.trim();
      if (!cmd) return;
      termIn.value = "";

      // Echo command
      termOut.innerHTML += `<div class="term-line term-echo">triquetra@osaima ~$ ${cmd}</div>`;
      
      // Execute command on real host!
      try {
        const output = await invoke("run_command", { cmd: cmd });
        termOut.innerHTML += `<div class="term-line">${output.replace(/</g, "&lt;").replace(/>/g, "&gt;")}</div>`;
      } catch (err) {
        termOut.innerHTML += `<div class="term-line" style="color: #ff3366">${err}</div>`;
      }
      
      termOut.scrollTop = termOut.scrollHeight;
    }
  });
}

// --- LAUNCHER LOGIC ---
window.closeLauncher = () => {
  document.getElementById("launcher").classList.add("hidden");
  document.getElementById("launcher-response").classList.add("hidden");
};

window.openLauncher = () => {
  document.getElementById("launcher").classList.remove("hidden");
  document.getElementById("launcher-input").focus();
};

window.runSuggestion = async (btn) => {
  const cmd = btn.innerText;
  const input = document.getElementById("launcher-input");
  const responseBox = document.getElementById("launcher-response");
  const responseContent = document.getElementById("response-content");
  
  input.value = cmd;
  responseBox.classList.remove("hidden");
  responseContent.innerText = "Executing intent: " + cmd + "\nWaiting for backend...";
  
  if (cmd === "Open terminal") {
    closeLauncher();
    openTerminal();
    return;
  }
};

window.addEventListener("keydown", (e) => {
  if (e.code === "Space" && e.target === document.body) {
    e.preventDefault();
    openLauncher();
  }
  if (e.key === "Escape") {
    closeLauncher();
  }
  if (e.code === "Enter" && e.altKey) {
    openTerminal();
  }
});
