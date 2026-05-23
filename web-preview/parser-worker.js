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
