document.addEventListener('DOMContentLoaded', () => {
    const dropZone = document.getElementById('drop-zone');
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
        e.preventDefault();
        dropZone.classList.add('dragover');
    });

    dropZone.addEventListener('dragleave', () => {
        dropZone.classList.remove('dragover');
    });

    dropZone.addEventListener('drop', (e) => {
        e.preventDefault();
        dropZone.classList.remove('dragover');
        if (e.dataTransfer.files && e.dataTransfer.files.length > 0) {
            processSelectedFile(e.dataTransfer.files[0]);
        }
    });

    // Handle click to browse
    dropZone.addEventListener('click', () => {
        fileInput.click();
    });

    fileInput.addEventListener('change', (e) => {
        if (e.target.files && e.target.files.length > 0) {
            processSelectedFile(e.target.files[0]);
        }
    });

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
                            currentTable = match[1].replace(/[`"\[\]]/g, '');
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
                                const columnName = columnMatch[1].replace(/[`"\[\]]/g, '');
                                
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

                    reader.onerror = function(e) {
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
});
