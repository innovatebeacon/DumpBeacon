document.addEventListener('DOMContentLoaded', () => {
    const dropZone = document.getElementById('drop-zone');
    
    // --- TAURI NATIVE INTEGRATION FOR MASSIVE 5GB+ FILES ---
    if (window.__TAURI__) {
        const { listen } = window.__TAURI__.event;
        const { invoke } = window.__TAURI__.core;


        const handleTauriDrop = (e) => {
            console.log("TAURI DROP EVENT FIRED:", e);
            const payload = e.payload;
            
            // Handle different Tauri v1/v2 payload structures safely
            let paths = null;
            if (Array.isArray(payload)) {
                paths = payload; // v1 structure
            } else if (payload && Array.isArray(payload.paths)) {
                paths = payload.paths; // v2 structure
            } else if (payload && payload.path) {
                paths = [payload.path];
            }
            
            if (paths && paths.length > 0) {
                processTauriFile(paths[0]);
            }
        };

        // Bind to all known Tauri file drop event names across v1 and v2!
        listen('tauri://drag-drop', handleTauriDrop);
        listen('tauri://drop', handleTauriDrop);

        // Power User Bridge
        window.exportConfig = async function(rules) {
            try {
                await invoke('export_rules', { rules: rules });
                alert("Rules exported successfully!");
            } catch (e) {
                if (e !== "Export cancelled") {
                    alert("Failed to export rules: " + e);
                }
            }
        };

        window.importConfig = async function() {
            try {
                const [filename, rules] = await invoke('import_rules');
                alert("Rules imported successfully!");
                window.updateProfileUI(filename);
                return rules;
            } catch (e) {
                if (e !== "Import cancelled") {
                    alert("Failed to import rules: " + e);
                }
                return null;
            }
        };

        window.updateProfileUI = function(filename) {
            const profileName = document.getElementById('profile-name');
            const profileBadge = document.getElementById('profile-badge');
            if (profileName) {
                profileName.textContent = filename || "Default Security Profile";
                if (filename) {
                    profileBadge.classList.replace('bg-blue-900/30', 'bg-emerald-900/30');
                    profileBadge.classList.replace('border-blue-500/30', 'border-emerald-500/30');
                    profileBadge.classList.replace('text-blue-300', 'text-emerald-300');
                } else {
                    profileBadge.classList.replace('bg-emerald-900/30', 'bg-blue-900/30');
                    profileBadge.classList.replace('border-emerald-500/30', 'border-blue-500/30');
                    profileBadge.classList.replace('text-emerald-300', 'text-blue-300');
                }
            }
        };
        listen('tauri://file-drop', handleTauriDrop);
    }
    const fileInput = document.getElementById('file-input');
    const resultsContainer = document.getElementById('results-container');
    const totalTablesEl = document.getElementById('total-tables');
    const totalColumnsEl = document.getElementById('total-columns');
    const jsonOutputEl = document.getElementById('json-output');
    const loadingEl = document.getElementById('loading');
    const fileNameEl = document.getElementById('file-name');
    const fileSizeEl = document.getElementById('file-size');

    // Handle drag and drop visual cues
    dropZone.addEventListener('dragover', (e) => {
        // We MUST NOT call preventDefault in Tauri, otherwise WebView2 consumes the OS drop event!
        if (!window.__TAURI__) {
            e.preventDefault();
        }
        dropZone.classList.add('dragover');
    });

    dropZone.addEventListener('dragleave', () => {
        dropZone.classList.remove('dragover');
    });

    dropZone.addEventListener('drop', (e) => {
        if (!window.__TAURI__) {
            e.preventDefault();
        }
        dropZone.classList.remove('dragover');
        
        // If we are in Tauri, ignore the HTML5 drop because the native 'tauri://drop' event handles it!
        // HTML5 drop objects do not contain absolute paths required for 5GB Rust streaming.
        if (window.__TAURI__) return;
        
        if (e.dataTransfer.files && e.dataTransfer.files.length > 0) {
            processSelectedFile(e.dataTransfer.files[0]);
        }
    });

    // Handle click to browse
    dropZone.addEventListener('click', () => {
        if (window.__TAURI__) {
            // Use our newly bound Native Rust Dialog
            window.__TAURI__.core.invoke('open_file_dialog')
                .then(absolutePath => {
                    if (absolutePath) {
                        processTauriFile(absolutePath);
                    }
                })
                .catch(e => {
                    if (e !== "No file selected") {
                        alert("Error opening file dialog: " + e);
                    }
                });
            return;
        }
        fileInput.click();
    });

    fileInput.addEventListener('change', (e) => {
        if (e.target.files && e.target.files.length > 0) {
            processSelectedFile(e.target.files[0]);
        }
    });



        async function processTauriFile(absolutePath) {
            if (!window.__TAURI__) return;
            const { invoke } = window.__TAURI__.core;
            const { listen } = window.__TAURI__.event;
            
            const progressBar = document.getElementById('progress-bar');
            const progressText = document.getElementById('progress-text');
            const cancelBtn = document.getElementById('cancel-btn');
            let unlistenProgress = null;
            let unlistenBatch = null;
            let isCancelled = false;

        fileNameEl.textContent = absolutePath;
        fileSizeEl.textContent = "Processing...";
        
        try {
            const isDir = await invoke('is_dir', { path: absolutePath });
            
            // Auto-generate the output path to skip the confusing second popup
            let outputPath = "";
            if (isDir) {
                outputPath = absolutePath + "_sanitized";
            } else {
                const lastDot = absolutePath.lastIndexOf('.');
                const lastSlash = Math.max(absolutePath.lastIndexOf('/'), absolutePath.lastIndexOf('\\'));
                if (lastDot !== -1 && lastDot > lastSlash) {
                    outputPath = absolutePath.substring(0, lastDot) + "_sanitized" + absolutePath.substring(lastDot);
                } else {
                    outputPath = absolutePath + "_sanitized";
                }
            }
            
            // Reset UI
            resultsContainer.classList.add('hidden');
            resultsContainer.classList.remove('opacity-100');
            loadingEl.classList.remove('hidden');
            totalTablesEl.textContent = '0';
            totalColumnsEl.textContent = '0';
            jsonOutputEl.textContent = '';
            if (progressBar) progressBar.style.width = '0%';
            if (progressText) progressText.textContent = '0%';
            
            if (cancelBtn) cancelBtn.classList.remove('hidden');
            const batchMsgEl = document.getElementById('batch-status-msg');
            if (batchMsgEl) {
                batchMsgEl.classList.add('hidden');
                if (isDir) {
                    batchMsgEl.classList.remove('hidden');
                    batchMsgEl.textContent = "Scanning directory...";
                }
            }

            unlistenProgress = await listen('progress-update', (event) => {
                const progress = event.payload;
                if (progressBar) progressBar.style.width = `${progress}%`;
                if (progressText) progressText.textContent = `${progress.toFixed(2)}%`;
            });

            unlistenBatch = await listen('batch-status', (event) => {
                if (batchMsgEl) {
                    batchMsgEl.textContent = event.payload;
                    batchMsgEl.classList.remove('hidden');
                }
            });
            
            console.log("Triggering Rust sanitizer for:", absolutePath, "to", outputPath);
            
            const handleCancel = () => {
                isCancelled = true;
                invoke("cancel_job").then(() => {
                    if (cancelBtn) cancelBtn.classList.add('hidden');
                }).catch(e => {
                    console.error("Error cancelling job: ", e);
                });
            };
            if (cancelBtn) cancelBtn.onclick = handleCancel;
            
            const result = await invoke("run_sanitizer", {
                inputPath: absolutePath,
                outputPath: outputPath
            });
            
            if (unlistenProgress) unlistenProgress();
            if (unlistenBatch) unlistenBatch();
            if (cancelBtn) cancelBtn.classList.add('hidden');
            
            const schema = result.schema;
            const tableNames = Object.keys(schema);
            const totalTables = tableNames.length;
            let totalColumns = 0;
            tableNames.forEach(table => { totalColumns += schema[table].length; });

            animateValue(totalTablesEl, 0, totalTables, 1200);
            animateValue(totalColumnsEl, 0, totalColumns, 1200);
            
            const jsonString = JSON.stringify(schema, null, 2);
            jsonOutputEl.textContent = jsonString;
            Prism.highlightElement(jsonOutputEl);

            loadingEl.classList.add('hidden');
            
            const successMsg = document.getElementById('success-message');
            if (successMsg) successMsg.textContent = result.message;

            resultsContainer.classList.remove('hidden');
            resultsContainer.classList.add('flex');
            requestAnimationFrame(() => {
                resultsContainer.classList.add('opacity-100');
            });

        } catch (error) {
            if (unlistenProgress) unlistenProgress();
            if (unlistenBatch) unlistenBatch();
            if (cancelBtn) cancelBtn.classList.add('hidden');
            console.error("Sanitization Failed:", error);
            loadingEl.classList.add('hidden');
            if (error !== "No file selected" && error !== "No folder selected") {
                if (!isCancelled) {
                    alert("Backend Error: " + error);
                } else {
                    alert("Sanitization cancelled.");
                }
            }
        }
    }

    // Helper to format file size cleanly
    function formatFileSize(bytes) {
        if (bytes === 0) return '0 Bytes';
        const k = 1024;
        const sizes = ['Bytes', 'KB', 'MB', 'GB'];
        const i = Math.floor(Math.log(bytes) / Math.log(k));
        return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
    }

    // Main controller logic to manage the Web Worker
    function processSelectedFile(file) {
        // Update file info display
        fileNameEl.textContent = file.name;
        fileSizeEl.textContent = formatFileSize(file.size);

        // Transition UI state
        resultsContainer.classList.add('hidden');
        resultsContainer.classList.remove('opacity-100');
        loadingEl.classList.remove('hidden');

        // Reset text
        totalTablesEl.textContent = '0';
        totalColumnsEl.textContent = '0';
        jsonOutputEl.textContent = '';

        // Initialize Web Worker for parsing using an Inline Blob Worker to bypass file:/// CORS
        const workerFunction = function() {
            // Web Worker for highly optimized, chunked SQL parsing
            self.onmessage = function(e) {
                const file = e.data.file;
                parseFileChunked(file);
            };

            function parseFileChunked(file) {
                // 5MB chunk size for reading large files efficiently
                const chunkSize = 1024 * 1024 * 5; 
                let offset = 0;
                
                // The final schema object we are building
                let schema = {};
                
                // State variables to track across chunks
                let currentTable = null;
                let inTableDefinition = false;
                let remainder = '';

                // Regex to detect "CREATE TABLE `name` (" or "CREATE TABLE name ("
                const createTableRegex = /CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?([^\s(]+)/i;

                function processChunkText(text, isLastChunk) {
                    // Prepend any incomplete line from the previous chunk
                    const data = remainder + text;
                    const lines = data.split(/\r?\n/);
                    
                    // If it's not the last chunk, keep the last line as it might be cut off
                    if (!isLastChunk) {
                        remainder = lines.pop() || '';
                    } else {
                        remainder = '';
                    }

                    for (let i = 0; i < lines.length; i++) {
                        let line = lines[i].trim();
                        
                        // Fast skips
                        if (!line || line.startsWith('--') || line.startsWith('/*')) {
                            continue;
                        }

                        // Detect table creation start
                        const match = line.match(createTableRegex);
                        if (match) {
                            // Strip structural characters like backticks, quotes, brackets
                            currentTable = match[1].replace(/[`"[\]]/g, '');
                            if (!schema[currentTable]) {
                                schema[currentTable] = [];
                            }
                            inTableDefinition = true;
                            continue;
                        }

                        // Extract columns if we are currently inside a CREATE TABLE block
                        if (inTableDefinition) {
                            // Detect end of block
                            if (line.startsWith(')') || line.startsWith('}')) {
                                inTableDefinition = false;
                                currentTable = null;
                                continue;
                            }

                            const upperLine = line.toUpperCase();
                            // Skip common constraint and index keywords
                            if (upperLine.startsWith('PRIMARY') || 
                                upperLine.startsWith('FOREIGN') || 
                                upperLine.startsWith('UNIQUE') || 
                                upperLine.startsWith('KEY') || 
                                upperLine.startsWith('INDEX') ||
                                upperLine.startsWith('CONSTRAINT') ||
                                upperLine.startsWith('CHECK')) {
                                continue;
                            }

                            // Match the first word on the line as the column name
                            const columnMatch = line.match(/^([^\s]+)/);
                            if (columnMatch) {
                                const columnName = columnMatch[1].replace(/[`"[\]]/g, '');
                                
                                // Final safeguard against reserved keywords being captured incorrectly
                                const skipWords = ['CONSTRAINT', 'PRIMARY', 'FOREIGN', 'UNIQUE', 'KEY', 'INDEX', 'CHECK'];
                                
                                if (!skipWords.includes(columnName.toUpperCase())) {
                                     if (!schema[currentTable].includes(columnName)) {
                                         schema[currentTable].push(columnName);
                                     }
                                }
                            }
                        }
                    }
                }

                function readNextChunk() {
                    if (offset >= file.size) {
                        // Processing complete, process any leftover text if any
                        if (remainder) {
                            processChunkText('', true);
                        }
                        // Send result back to the main thread
                        self.postMessage({ type: 'complete', schema: schema });
                        return;
                    }

                    const slice = file.slice(offset, offset + chunkSize);
                    const reader = new FileReader();

                    reader.onload = function(e) {
                        const text = e.target.result;
                        offset += chunkSize;
                        const isLastChunk = offset >= file.size;
                        
                        processChunkText(text, isLastChunk);
                        
                        // Use setTimeout to avoid synchronous call stack limits and keep worker responsive
                        setTimeout(readNextChunk, 0);
                    };

                    reader.onerror = function() {
                        self.postMessage({ type: 'error', message: 'Error reading file chunks.' });
                    };

                    // readAsText handles text decoding safely
                    reader.readAsText(slice);
                }

                // Start reading the first chunk
                readNextChunk();
            }
        };

        const workerCodeString = '(' + workerFunction.toString() + ')()';
        const blob = new Blob([workerCodeString], { type: 'application/javascript' });
        const workerUrl = URL.createObjectURL(blob);
        const worker = new Worker(workerUrl);

        // Send the File object directly to the worker
        worker.postMessage({ file: file });

        // Listen for worker messages
        worker.onmessage = (e) => {
            const data = e.data;
            
            if (data.type === 'complete') {
                const schema = data.schema;
                
                // Calculate aggregated metrics
                const tableNames = Object.keys(schema);
                const totalTables = tableNames.length;
                let totalColumns = 0;
                
                tableNames.forEach(table => {
                    totalColumns += schema[table].length;
                });

                // Smoothly animate the numeric values
                animateValue(totalTablesEl, 0, totalTables, 1200);
                animateValue(totalColumnsEl, 0, totalColumns, 1200);
                
                // Format schema object into a beautiful string and apply Prism highlight
                const jsonString = JSON.stringify(schema, null, 2);
                jsonOutputEl.textContent = jsonString;
                Prism.highlightElement(jsonOutputEl);

                // UI Transitions to display results
                loadingEl.classList.add('hidden');
                resultsContainer.classList.remove('hidden');
                
                // A slight delay to ensure 'hidden' class removal is processed before opacity
                requestAnimationFrame(() => {
                    resultsContainer.classList.add('opacity-100');
                });
                
                // Cleanup worker
                worker.terminate();
            } else if (data.type === 'error') {
                loadingEl.classList.add('hidden');
                alert('An error occurred during parsing: ' + data.message);
                worker.terminate();
            }
        };

        worker.onerror = (error) => {
            loadingEl.classList.add('hidden');
            alert('Worker initialization error: ' + error.message);
            worker.terminate();
        };
    }

    // Beautiful number increment animation
    function animateValue(obj, start, end, duration) {
        let startTimestamp = null;
        const step = (timestamp) => {
            if (!startTimestamp) startTimestamp = timestamp;
            const progress = Math.min((timestamp - startTimestamp) / duration, 1);
            // using easeOutQuad
            const easeProgress = progress * (2 - progress);
            obj.innerHTML = Math.floor(easeProgress * (end - start) + start);
            if (progress < 1) {
                window.requestAnimationFrame(step);
            }
        };
        window.requestAnimationFrame(step);
    }
    // --- Settings Modal Logic ---
    const settingsModal = document.getElementById('settings-modal');
    const openSettingsBtn = document.getElementById('open-settings-btn');
    const closeSettingsBtn = document.getElementById('close-settings-btn');
    const rulesList = document.getElementById('rules-list');
    const addRuleBtn = document.getElementById('add-rule-btn');
    const exportRulesBtn = document.getElementById('export-rules-btn');
    const importRulesBtn = document.getElementById('import-rules-btn');

    let currentRules = { column_rules: [], regex_rules: [] };

    function renderRules() {
        if (!rulesList) return;
        rulesList.innerHTML = '';
        currentRules.column_rules.forEach((rule, index) => {
            const ruleEl = document.createElement('div');
            ruleEl.className = 'bg-gray-800 border border-gray-700 rounded-lg p-4 flex justify-between items-center';
            ruleEl.innerHTML = `
                <div>
                    <h4 class="text-white font-bold">${rule.name}</h4>
                    <p class="text-sm text-gray-400">Keywords: ${rule.keywords.join(', ')}</p>
                    <p class="text-sm text-blue-400 mt-1">Placeholder: ${rule.placeholder}</p>
                </div>
                <button class="text-red-400 hover:text-red-300 delete-rule-btn" data-index="${index}">
                    <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"></path></svg>
                </button>
            `;
            rulesList.appendChild(ruleEl);
        });

        document.querySelectorAll('.delete-rule-btn').forEach(btn => {
            btn.addEventListener('click', (e) => {
                const idx = parseInt(e.currentTarget.getAttribute('data-index'));
                currentRules.column_rules.splice(idx, 1);
                renderRules();
            });
        });
    }

    if (openSettingsBtn && settingsModal) {
        openSettingsBtn.addEventListener('click', async () => {
            settingsModal.classList.remove('hidden');
            if (window.__TAURI__) {
                try {
                    const { invoke } = window.__TAURI__.core;
                    currentRules = await invoke('get_current_rules');
                    renderRules();
                } catch (e) {
                    console.error("Failed to fetch rules", e);
                }
            }
        });
    }

    if (closeSettingsBtn && settingsModal) {
        closeSettingsBtn.addEventListener('click', () => {
            settingsModal.classList.add('hidden');
        });
    }

    if (addRuleBtn) {
        addRuleBtn.addEventListener('click', () => {
            const nameInput = document.getElementById('new-rule-name');
            const keywordsInput = document.getElementById('new-rule-keywords');
            const placeholderInput = document.getElementById('new-rule-placeholder');

            if (!nameInput.value || !keywordsInput.value || !placeholderInput.value) {
                alert("Please fill in all rule fields.");
                return;
            }

            const keywords = keywordsInput.value.split(',').map(k => k.trim()).filter(k => k.length > 0);

            currentRules.column_rules.push({
                name: nameInput.value,
                keywords: keywords,
                placeholder: placeholderInput.value
            });

            nameInput.value = '';
            keywordsInput.value = '';
            placeholderInput.value = '';
            renderRules();
        });
    }

    if (exportRulesBtn) {
        exportRulesBtn.addEventListener('click', async () => {
            if (window.exportConfig) {
                await window.exportConfig(currentRules);
            }
        });
    }

    if (importRulesBtn) {
        importRulesBtn.addEventListener('click', async () => {
            if (window.importConfig) {
                const rules = await window.importConfig();
                if (rules) {
                    currentRules = rules;
                    renderRules();
                }
            }
        });
    }

    // --- Preview Modal Logic ---
    const previewModal = document.getElementById('preview-modal');
    const openPreviewBtn = document.getElementById('open-preview-btn');
    const closePreviewBtn = document.getElementById('close-preview-btn');
    const previewOriginalContent = document.getElementById('preview-original-content');
    const previewMaskedContent = document.getElementById('preview-masked-content');
    const previewLoading = document.getElementById('preview-loading');

    // Helper to safely escape HTML
    function escapeHTML(str) {
        return str.replace(/[&<>'"]/g, 
            tag => ({
                '&': '&amp;',
                '<': '&lt;',
                '>': '&gt;',
                "'": '&#39;',
                '"': '&quot;'
            }[tag])
        );
    }

    if (openPreviewBtn && previewModal) {
        openPreviewBtn.addEventListener('click', async () => {
            if (!window.__TAURI__) return;
            
            try {
                const { invoke } = window.__TAURI__.core;
                const path = await invoke('open_file_dialog');
                
                previewModal.classList.remove('hidden');
                previewLoading.classList.remove('hidden');
                previewOriginalContent.innerHTML = '';
                previewMaskedContent.innerHTML = '';
                
                const pairs = await invoke('preview_masking', { path: path });
                previewLoading.classList.add('hidden');
                
                if (pairs.length === 0) {
                    previewOriginalContent.innerHTML = '<div class="text-gray-500 italic">No sensitive lines found to mask.</div>';
                    return;
                }
                
                pairs.forEach(([original, masked]) => {
                    const origDiv = document.createElement('div');
                    origDiv.className = 'bg-red-900/20 border border-red-500/20 p-2 rounded whitespace-pre-wrap break-all';
                    origDiv.innerHTML = escapeHTML(original);
                    previewOriginalContent.appendChild(origDiv);
                    
                    const maskedDiv = document.createElement('div');
                    maskedDiv.className = 'bg-emerald-900/20 border border-emerald-500/20 p-2 rounded whitespace-pre-wrap break-all';
                    
                    // Simple highlighting for [MASKED_...] patterns
                    let htmlMasked = escapeHTML(masked);
                    htmlMasked = htmlMasked.replace(/(\[MASKED_[A-Z_]+\])/g, '<b class="text-emerald-400 font-bold bg-emerald-900/40 px-1 rounded">$1</b>');
                    
                    maskedDiv.innerHTML = htmlMasked;
                    previewMaskedContent.appendChild(maskedDiv);
                });
                
            } catch (e) {
                if (e !== "No file selected") {
                    alert("Preview Error: " + e);
                }
                previewModal.classList.add('hidden');
                previewLoading.classList.add('hidden');
            }
        });
    }

    if (closePreviewBtn && previewModal) {
        closePreviewBtn.addEventListener('click', () => {
            previewModal.classList.add('hidden');
        });
    }

});
