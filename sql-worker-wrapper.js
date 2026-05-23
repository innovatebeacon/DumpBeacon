(function () {
  "use strict";

  const workerSource = `
    const decoder = new TextDecoder();
    let tables = [];
    let active = null;
    let statement = "";
    let inCreateTable = false;
    let parenDepth = 0;
    let bytesRead = 0;
    let aborted = false;

    self.onmessage = async (event) => {
      if (event.data.type === "abort") {
        aborted = true;
        return;
      }

      if (event.data.type !== "parse") return;

      reset();
      const file = event.data.file;

      try {
        await streamFile(file);
        if (statement.trim()) parseStatement(statement);
        postMessage({ type: "done", tables, bytesRead: file.size });
      } catch (error) {
        if (!aborted) {
          postMessage({ type: "error", message: error.message || String(error) });
        }
      }
    };

    function reset() {
      tables = [];
      active = null;
      statement = "";
      inCreateTable = false;
      parenDepth = 0;
      bytesRead = 0;
      aborted = false;
    }

    async function streamFile(file) {
      const reader = file.stream().getReader();
      let carry = "";

      while (!aborted) {
        const { value, done } = await reader.read();
        if (done) break;

        bytesRead += value.byteLength;
        const text = decoder.decode(value, { stream: true });
        const lines = (carry + text).split(/\\r?\\n/);
        carry = lines.pop() || "";

        for (const line of lines) {
          consumeLine(line);
        }

        postMessage({
          type: "progress",
          bytesRead,
          totalBytes: file.size,
          tableCount: tables.length,
          columnCount: countColumns()
        });
      }

      const finalText = decoder.decode();
      if (finalText || carry) consumeLine(carry + finalText);
    }

    function consumeLine(rawLine) {
      const line = stripLineComment(rawLine);
      if (!inCreateTable) {
        const start = line.search(/\\bCREATE\\s+(?:TEMPORARY\\s+)?TABLE\\b/i);
        if (start === -1) return;
        inCreateTable = true;
        statement = line.slice(start) + "\\n";
        parenDepth = depthDelta(line);
        if (parenDepth <= 0 && /;\\s*$/.test(line)) finishStatement();
        return;
      }

      statement += line + "\\n";
      parenDepth += depthDelta(line);

      if (parenDepth <= 0 && /;\\s*$/.test(line)) {
        finishStatement();
      }
    }

    function finishStatement() {
      parseStatement(statement);
      statement = "";
      inCreateTable = false;
      parenDepth = 0;
    }

    function stripLineComment(line) {
      let quote = "";
      for (let index = 0; index < line.length - 1; index += 1) {
        const char = line[index];
        const next = line[index + 1];
        if (quote) {
          if (char === quote && line[index - 1] !== "\\\\") quote = "";
          continue;
        }
        if (char === "'" || char === '"' || char === "\`") {
          quote = char;
          continue;
        }
        if (char === "-" && next === "-") return line.slice(0, index);
      }
      return line;
    }

    function depthDelta(line) {
      let quote = "";
      let delta = 0;
      for (let index = 0; index < line.length; index += 1) {
        const char = line[index];
        if (quote) {
          if (char === quote && line[index - 1] !== "\\\\") quote = "";
          continue;
        }
        if (char === "'" || char === '"' || char === "\`") {
          quote = char;
        } else if (char === "(") {
          delta += 1;
        } else if (char === ")") {
          delta -= 1;
        }
      }
      return delta;
    }

    function parseStatement(sql) {
      const normalized = sql.replace(/\\/\\*[\\s\\S]*?\\*\\//g, " ");
      const match = normalized.match(/CREATE\\s+(?:TEMPORARY\\s+)?TABLE\\s+(?:IF\\s+NOT\\s+EXISTS\\s+)?((?:["'\\x60]?[^"'\\x60\\s.(]+["'\\x60]?\\.)?["'\\x60]?[^"'\\x60\\s(]+["'\\x60]?)[\\s\\n]*\\(/i);
      if (!match) return;

      const tableName = cleanIdentifier(match[1]);
      const openIndex = normalized.indexOf("(", match.index + match[0].length - 1);
      const closeIndex = findMatchingParen(normalized, openIndex);
      if (openIndex === -1 || closeIndex === -1) return;

      const body = normalized.slice(openIndex + 1, closeIndex);
      const columns = splitTopLevel(body)
        .map(parseColumn)
        .filter(Boolean);

      tables.push({
        name: tableName,
        columns
      });
    }

    function findMatchingParen(text, openIndex) {
      let quote = "";
      let depth = 0;
      for (let index = openIndex; index < text.length; index += 1) {
        const char = text[index];
        if (quote) {
          if (char === quote && text[index - 1] !== "\\\\") quote = "";
          continue;
        }
        if (char === "'" || char === '"' || char === "\`") {
          quote = char;
        } else if (char === "(") {
          depth += 1;
        } else if (char === ")") {
          depth -= 1;
          if (depth === 0) return index;
        }
      }
      return -1;
    }

    function splitTopLevel(body) {
      const parts = [];
      let quote = "";
      let depth = 0;
      let start = 0;

      for (let index = 0; index < body.length; index += 1) {
        const char = body[index];
        if (quote) {
          if (char === quote && body[index - 1] !== "\\\\") quote = "";
          continue;
        }
        if (char === "'" || char === '"' || char === "\`") {
          quote = char;
        } else if (char === "(") {
          depth += 1;
        } else if (char === ")") {
          depth -= 1;
        } else if (char === "," && depth === 0) {
          parts.push(body.slice(start, index).trim());
          start = index + 1;
        }
      }

      const last = body.slice(start).trim();
      if (last) parts.push(last);
      return parts;
    }

    function parseColumn(definition) {
      if (!definition || /^(CONSTRAINT|PRIMARY|FOREIGN|UNIQUE|KEY|INDEX|CHECK|EXCLUDE)\\b/i.test(definition)) {
        return null;
      }

      const match = definition.match(/^("[^"]+"|'[^']+'|\\x60[^\\x60]+\\x60|\\[[^\\]]+\\]|\\S+)\\s+([\\s\\S]+)$/);
      if (!match) return null;

      const name = cleanIdentifier(match[1]);
      const rest = match[2].trim().replace(/,\\s*$/, "");
      const typeMatch = rest.match(/^(.+?)(?=\\s+(?:COLLATE|CONSTRAINT|DEFAULT|GENERATED|IDENTITY|NOT\\s+NULL|NULL|PRIMARY|REFERENCES|UNIQUE|CHECK)\\b|$)/i);
      const type = (typeMatch ? typeMatch[1] : rest).trim();

      return {
        name,
        type,
        nullable: !/\\bNOT\\s+NULL\\b/i.test(rest),
        primaryKey: /\\bPRIMARY\\s+KEY\\b/i.test(rest),
        unique: /\\bUNIQUE\\b/i.test(rest),
        default: extractDefault(rest)
      };
    }

    function extractDefault(definition) {
      const match = definition.match(/\\bDEFAULT\\s+((?:'[^']*')|(?:"[^"]*")|(?:\\([^)]*\\))|[^\\s,]+)/i);
      return match ? match[1] : null;
    }

    function cleanIdentifier(identifier) {
      return identifier
        .split(".")
        .map((part) => part.trim().replace(/^(["'\\x60\\[])(.*)(["'\\x60\\]])$/, "$2"))
        .join(".");
    }

    function countColumns() {
      return tables.reduce((sum, table) => sum + table.columns.length, 0);
    }
  `;

  const workerUrl = URL.createObjectURL(new Blob([workerSource], { type: "text/javascript" }));
  let worker = null;
  let currentTables = [];

  const dropzone = document.getElementById("dropzone");
  const fileInput = document.getElementById("fileInput");
  const cancelButton = document.getElementById("cancelButton");
  const copyButton = document.getElementById("copyButton");
  const fileName = document.getElementById("fileName");
  const progress = document.getElementById("progress");
  const statusText = document.getElementById("statusText");
  const warningText = document.getElementById("warningText");
  const tableCount = document.getElementById("tableCount");
  const columnCount = document.getElementById("columnCount");
  const bytesRead = document.getElementById("bytesRead");
  const tableList = document.getElementById("tableList");
  const jsonOutput = document.getElementById("jsonOutput");

  fileInput.addEventListener("change", () => {
    const file = fileInput.files && fileInput.files[0];
    if (file) parseFile(file);
  });

  cancelButton.addEventListener("click", (event) => {
    event.preventDefault();
    abortWorker();
    statusText.textContent = "Parsing cancelled.";
  });

  copyButton.addEventListener("click", async () => {
    await navigator.clipboard.writeText(JSON.stringify(currentTables, null, 2));
    copyButton.textContent = "Copied";
    setTimeout(() => {
      copyButton.textContent = "Copy JSON";
    }, 1200);
  });

  ["dragenter", "dragover"].forEach((eventName) => {
    dropzone.addEventListener(eventName, (event) => {
      event.preventDefault();
      dropzone.classList.add("dragging");
    });
  });

  ["dragleave", "drop"].forEach((eventName) => {
    dropzone.addEventListener(eventName, () => {
      dropzone.classList.remove("dragging");
    });
  });

  dropzone.addEventListener("drop", (event) => {
    event.preventDefault();
    const file = event.dataTransfer.files && event.dataTransfer.files[0];
    if (file) parseFile(file);
  });

  function parseFile(file) {
    abortWorker();
    resetUi(file);

    worker = new Worker(workerUrl);
    worker.onmessage = ({ data }) => {
      if (data.type === "progress") {
        updateProgress(data);
      } else if (data.type === "done") {
        currentTables = data.tables;
        renderResults(currentTables);
        updateProgress({ bytesRead: data.bytesRead, totalBytes: file.size });
        statusText.textContent = "Parsing complete.";
        cancelButton.disabled = true;
        copyButton.disabled = currentTables.length === 0;
        worker.terminate();
        worker = null;
      } else if (data.type === "error") {
        warningText.textContent = data.message;
        statusText.textContent = "Parsing failed.";
        cancelButton.disabled = true;
      }
    };

    worker.postMessage({ type: "parse", file });
  }

  function abortWorker() {
    if (!worker) return;
    worker.postMessage({ type: "abort" });
    worker.terminate();
    worker = null;
    cancelButton.disabled = true;
  }

  function resetUi(file) {
    currentTables = [];
    fileName.textContent = file.name + " (" + formatBytes(file.size) + ")";
    progress.value = 0;
    statusText.textContent = "Parsing...";
    warningText.textContent = "";
    tableCount.textContent = "0";
    columnCount.textContent = "0";
    bytesRead.textContent = "0%";
    tableList.innerHTML = "";
    jsonOutput.textContent = "[]";
    copyButton.disabled = true;
    cancelButton.disabled = false;
  }

  function updateProgress(data) {
    const percent = data.totalBytes ? Math.min(100, Math.round((data.bytesRead / data.totalBytes) * 100)) : 0;
    progress.value = percent;
    bytesRead.textContent = percent + "%";
    if (typeof data.tableCount === "number") tableCount.textContent = String(data.tableCount);
    if (typeof data.columnCount === "number") columnCount.textContent = String(data.columnCount);
  }

  function renderResults(tables) {
    tableCount.textContent = String(tables.length);
    columnCount.textContent = String(tables.reduce((sum, table) => sum + table.columns.length, 0));
    jsonOutput.textContent = JSON.stringify(tables, null, 2);

    if (tables.length === 0) {
      tableList.innerHTML = "<p>No CREATE TABLE statements were detected.</p>";
      return;
    }

    tableList.replaceChildren(...tables.map(renderTable));
  }

  function renderTable(table) {
    const article = document.createElement("article");
    const title = document.createElement("div");
    title.className = "table-title";
    title.innerHTML = "<strong><code></code></strong><span class='tag'></span>";
    title.querySelector("code").textContent = table.name;
    title.querySelector(".tag").textContent = table.columns.length + " columns";

    const grid = document.createElement("table");
    grid.innerHTML = "<thead><tr><th>Name</th><th>Type</th><th>Flags</th></tr></thead><tbody></tbody>";
    const body = grid.querySelector("tbody");

    table.columns.forEach((column) => {
      const row = document.createElement("tr");
      const flags = [
        column.nullable ? "nullable" : "not null",
        column.primaryKey ? "primary key" : "",
        column.unique ? "unique" : "",
        column.default ? "default " + column.default : ""
      ].filter(Boolean).join(", ");
      row.innerHTML = "<td><code></code></td><td></td><td></td>";
      row.children[0].querySelector("code").textContent = column.name;
      row.children[1].textContent = column.type;
      row.children[2].textContent = flags;
      body.append(row);
    });

    article.append(title, grid);
    return article;
  }

  function formatBytes(bytes) {
    if (bytes < 1024) return bytes + " B";
    const units = ["KB", "MB", "GB"];
    let size = bytes / 1024;
    let unit = 0;
    while (size >= 1024 && unit < units.length - 1) {
      size /= 1024;
      unit += 1;
    }
    return size.toFixed(size >= 10 ? 1 : 2) + " " + units[unit];
  }
})();
