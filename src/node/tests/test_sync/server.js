/**
 * Node Watcher HTTP Server
 * 
 * A monitoring server for automated testing of node synchronization.
 * 
 * Features:
 * - Automatically runs test sequences on startup
 * - Monitor sync status via metrics endpoint
 * - View server logs
 * - Generate test reports
 * - Health status endpoint
 * 
 * Environment Variables:
 * - NODE_WATCHER_HTTP: Server address (e.g., 127.0.0.1:3000)
 * - SERVER_IP: Metrics server IP (default: 127.0.0.1)
 */

const http = require('http');
const https = require('https');
const fs = require('fs').promises;
const path = require('path');
const { spawn } = require('child_process');
const util = require('util');
const execPromise = util.promisify(require('child_process').exec);

// Simple HTML escaping helper to safely render text in HTML contexts.
// Escapes only the characters that are significant in HTML, preserving
// whitespace and newlines for use inside <pre> blocks.
function escapeHtml(str) {
  if (str === null || str === undefined) return '';
  return String(str)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

// ===================================================================
// CONFIGURATION
// ===================================================================

const LOG_FILE = '/logs/node-watcher.log';
const NODE_LOG_FILE = '/logs/output.log';
const DB_PATH = '/db';
const NODE_STOP_TIMEOUT = 60000; // 1 minute in milliseconds
const NODE_STABILITY_CHECK_DELAY = 5000; // 5 seconds
const SYNC_WAIT_TIMEOUT = 6 * 60 * 60 * 1000; // 6 hours in milliseconds
const NODE_RUN_ARGS = process.env.NODE_RUN_ARGS && process.env.NODE_RUN_ARGS.trim() !== '' ? process.env.NODE_RUN_ARGS.trim().split(/\s+/) : ['-c', '/main']; // Base arguments to run the node

// Get SERVER_IP from environment variable
const SERVER_IP = process.env.SERVER_IP || '127.0.0.1';

// Slack webhook URL (set via environment variables)
const SLACK_WEBHOOK_URL = process.env.SLACK_WEBHOOK_URL || '';
const SLACK_BOT_TOKEN = process.env.SLACK_BOT_TOKEN || '';
const SLACK_CHANNEL_ID = process.env.SLACK_CHANNEL_ID || '';

// ===================================================================
// GLOBAL STATE
let FormData;
let nodePid = null;
let nodeStartTime = null;
let syncCheckRunning = false;
let syncWaiters = [];
let isSynced = false;
let testResults = null;

class SyncTimeoutError extends Error {
  constructor(message) {
    super(message);
    this.name = 'SyncTimeoutError';
  }
}

// Global config values for network and node_id
let GLOBAL_NETWORK = process.env.SYNC_TEST_NETWORK && process.env.SYNC_TEST_NETWORK !== '' ? process.env.SYNC_TEST_NETWORK : 'unknown-net';
let GLOBAL_NODE_ID = process.env.SYNC_TEST_NODE_ID && process.env.SYNC_TEST_NODE_ID !== '' ? process.env.SYNC_TEST_NODE_ID : 'unknown-node';

// ===================================================================
// UTILITY FUNCTIONS
// ===================================================================

function normalizePid(pid) {
  const pidNum = typeof pid === 'number' ? pid : Number(pid);
  if (!Number.isInteger(pidNum) || pidNum <= 0) {
    return null;
  }
  return pidNum;
}

function isProcessRunning(pid) {
  const pidNum = normalizePid(pid);
  if (pidNum === null) {
    return false;
  }
  try {
    process.kill(pidNum, 0);
    return true;
  } catch (error) {
    return false;
  }
}

async function stopProcessWithTimeout(pid) {
  const pidNum = normalizePid(pid);
  if (pidNum === null) {
    await log(`Invalid PID '${pid}', cannot stop process`);
    return false;
  }

  try {
    process.kill(pidNum, 'SIGTERM');
    await log(`Sent SIGTERM to process ${pidNum}`);
  } catch (error) {
    if (error && error.code === 'ESRCH') {
      await log(`Process ${pidNum} is already stopped`);
      return true;
    }
    await log(`Failed to send SIGTERM to process ${pidNum}: ${error.message}`);
    return false;
  }

  const stopStartTime = Date.now();
  while (Date.now() - stopStartTime < NODE_STOP_TIMEOUT) {
    if (!isProcessRunning(pidNum)) {
      await log(`Process ${pidNum} has stopped successfully`);
      return true;
    }
    await new Promise(resolve => setTimeout(resolve, 1000));
  }

  await log(`Process ${pidNum} did not stop after ${NODE_STOP_TIMEOUT / 1000} seconds, sending SIGKILL`);
  try {
    process.kill(pidNum, 'SIGKILL');
    await log(`Sent SIGKILL to process ${pidNum}`);
  } catch (error) {
    if (error && error.code === 'ESRCH') {
      await log(`Process ${pidNum} exited before SIGKILL was delivered`);
      return true;
    }
    await log(`Error sending SIGKILL to process ${pidNum}: ${error.message}`);
    return false;
  }

  // Give the OS a short window to reap the process after SIGKILL.
  const killConfirmDeadline = Date.now() + 5000;
  while (Date.now() < killConfirmDeadline) {
    if (!isProcessRunning(pidNum)) {
      await log(`Process ${pidNum} has stopped after SIGKILL`);
      return true;
    }
    await new Promise(resolve => setTimeout(resolve, 200));
  }

  await log(`WARNING: Process ${pidNum} is still running after SIGKILL attempt`);
  return false;
}

// Stop all running node processes (by name)
async function stopAllNodeProcesses() {
  try {
    const { stdout } = await execPromise('pgrep -f "ton-node" || true');
    const pids = stdout.trim().split(/\s+/).filter(pid => pid);
    if (pids.length > 0) {
      await log(`Found ${pids.length} running node process(es). Stopping all...`);
      for (const pidStr of pids) {
        const pidNumber = parseInt(pidStr, 10);
        if (!Number.isInteger(pidNumber) || pidNumber <= 0) {
          await log(`Skipping invalid PID from pgrep output: "${pidStr}"`);
          continue;
        }
        await stopProcessWithTimeout(pidNumber);
      }
    } else {
      await log('No running node processes found.');
    }
  } catch (e) {
    await log('Error checking/stopping node processes: ' + e.message);
  }
}

// Analyzes logs and returns {blocksAccepted, syncSpeed}
async function analyzeSyncLog(syncDurationSeconds) {
  let blocksAccepted = null;
  try {
    const { stdout: blocksStdout } = await execPromise(`cat "${NODE_LOG_FILE}" | grep "Applied master block" | wc -l`);
    blocksAccepted = parseInt(blocksStdout.trim(), 10);
    if (isNaN(blocksAccepted)) blocksAccepted = 0;
  } catch (err) {
    await log(`Could not count applied master blocks: ${err.message}`);
    blocksAccepted = null;
  }
  const syncSpeed = (blocksAccepted !== null && syncDurationSeconds > 0) ? Number((blocksAccepted / syncDurationSeconds).toFixed(2)) : null;
  return { blocksAccepted, syncSpeed };
}

// Logging function
async function log(message) {
  const timestamp = new Date().toISOString();
  const logMessage = `${timestamp} - ${message}\n`;
  
  // Write to file
  try {
    await fs.appendFile(LOG_FILE, logMessage);
  } catch (err) {
    console.error('Failed to write to log file:', err);
  }
  
  // Also output to console
  console.log(logMessage.trim());
}

// Helper function to send JSON response
function sendJsonResponse(res, statusCode, data) {
  res.writeHead(statusCode, { 'Content-Type': 'application/json' });
  res.end(JSON.stringify(data));
}

// Helper function to get uptime in seconds
function getUptime() {
  if (!nodeStartTime) return null;
  return Math.round((Date.now() - nodeStartTime) / 1000);
}

// ===================================================================
// SERVER CONFIGURATION
// ===================================================================

// Parse the NODE_WATCHER_HTTP environment variable
function parseAddress() {
  const address = process.env.NODE_WATCHER_HTTP;
  
  if (!address) {
    throw new Error('NODE_WATCHER_HTTP environment variable is not set. Example: NODE_WATCHER_HTTP=127.0.0.1:3000');
  }
  
  const [host, port] = address.split(':');
  
  if (!host || !port) {
    throw new Error('Invalid NODE_WATCHER_HTTP format. Expected: <ip>:<port>. Example: NODE_WATCHER_HTTP=127.0.0.1:3000');
  }
  
  const portNum = parseInt(port, 10);
  
  if (isNaN(portNum) || portNum < 1 || portNum > 65535) {
    throw new Error('Invalid port number in NODE_WATCHER_HTTP');
  }
  
  return { host, port: portNum };
}

// ===================================================================
// HTTP ENDPOINT HANDLERS
// ===================================================================

// Handler functions for each endpoint
// IMPORTANT: These handlers provide read-only access to monitoring data
// The server runs tests automatically on startup

async function handleLogs(req, res, queryParams) {
  await log(`Received /getlogs request${queryParams.size > 0 ? ` with params: ${JSON.stringify(Object.fromEntries(queryParams))}` : ''}`);
  
  try {
    // Get the 'last' parameter, default to 100, maximum 3000
    const lastParam = queryParams.get('last');
    let last = lastParam ? parseInt(lastParam, 10) : 100;
    
    // Validate the parameter
    if (isNaN(last) || last < 1) {
      sendJsonResponse(res, 400, { 
        status: 'error', 
        message: 'Invalid "last" parameter. Must be a positive number.',
        endpoint: '/getlogs'
      });
      return;
    }
    
    // Limit to maximum 3000 lines
    if (last > 3000) {
      last = 3000;
    }
    
    // Check if log file exists
    try {
      await fs.access(LOG_FILE);
    } catch (err) {
      sendJsonResponse(res, 200, { 
        status: 'success', 
        message: 'No logs available',
        lines: [],
        count: 0,
        endpoint: '/getlogs'
      });
      return;
    }
    
    // Use tail command to read log file efficiently
    let lines;
    let totalLines = null;
    
    try {
      // Return last N lines using tail (max 3000), with a 5s timeout
      const { stdout } = await execPromise(`timeout 5 tail -n ${last} "${LOG_FILE}"`);
      lines = stdout.split('\n').filter(line => line.trim() !== '');
      // Get total line count efficiently for reporting (timeout 5s)
      const { stdout: wcOutput } = await execPromise(`timeout 5 wc -l < "${LOG_FILE}"`);
      totalLines = parseInt(wcOutput.trim(), 10);
      if (isNaN(totalLines)) totalLines = lines.length;
    } catch (error) {
      await log(`Error reading log file with tail: ${error.message}`);
      sendJsonResponse(res, 500, { 
        status: 'error', 
        message: `Failed to read log file: ${error.message}`,
        endpoint: '/getlogs'
      });
      return;
    }
    
    sendJsonResponse(res, 200, { 
      status: 'success', 
      message: `Retrieved ${lines.length} log lines`,
      lines: lines,
      count: lines.length,
      total: totalLines,
      endpoint: '/getlogs'
    });
  } catch (error) {
    await log(`Error in /getlogs handler: ${error.message}`);
    sendJsonResponse(res, 500, { 
      status: 'error', 
      message: error.message,
      endpoint: '/getlogs'
    });
  }
}

async function handleReport(req, res, queryParams) {
  await log('Received /report request');
  
  try {
    if (!testResults) {
      sendJsonResponse(res, 200, {
        status: 'success',
        message: 'No test results available. Run tests first.',
        endpoint: '/report'
      });
      return;
    }
    
    // Generate HTML report
    const html = generateHtmlReport(testResults);
    
    res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
    res.end(html);
  } catch (error) {
    await log(`Error in /report handler: ${error.message}`);
    sendJsonResponse(res, 500, { 
      status: 'error', 
      message: error.message,
      endpoint: '/report'
    });
  }
}

function generateHtmlReport(results) {
  const isRunning = results.endTime === null;
  const totalDuration = isRunning ? Math.round((Date.now() - results.startTime) / 1000) : Math.round((results.endTime - results.startTime) / 1000);
  const successCount = results.cases.filter(c => c.status === 'SUCCESS').length;
  const failedCount = results.cases.filter(c => c.status === 'FAILED').length;
  const completedCount = successCount + failedCount;
  const passRate = completedCount > 0 ? ((successCount / completedCount) * 100).toFixed(1) : '0.0';
  
  let statusColor, statusText;
  if (isRunning) {
    statusColor = '#f59e0b'; // Orange/amber color for in progress
    statusText = 'IN PROGRESS';
  } else if (results.cases.length === 0) {
    statusColor = '#64748b'; // Gray for no tests
    statusText = 'NO TESTS';
  } else if (failedCount === 0) {
    statusColor = '#10b981'; // Green for all passed
    statusText = 'ALL PASSED';
  } else {
    statusColor = '#ef4444'; // Red for failures
    statusText = `${failedCount} FAILED`;
  }
  
  return `<!DOCTYPE html>
<html>
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Test Report - Node Watcher</title>
  <style>
    * { margin: 0; padding: 0; box-sizing: border-box; }
    body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; background: #0f172a; color: #e2e8f0; padding: 2rem; }
    .container { max-width: 1200px; margin: 0 auto; }
    .header { text-align: center; margin-bottom: 3rem; }
    .header h1 { font-size: 2.5rem; margin-bottom: 0.5rem; background: linear-gradient(135deg, #6366f1 0%, #8b5cf6 100%); -webkit-background-clip: text; -webkit-text-fill-color: transparent; }
    .header p { color: #94a3b8; font-size: 1.1rem; }
    .labels { color: #a5b4fc; font-size: 1.1rem; margin-top: 0.5rem; margin-bottom: 1.5rem; }
    .labels span { margin-right: 2rem; }
    .summary { display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 1.5rem; margin-bottom: 3rem; }
    .summary-card { background: #1e293b; border-radius: 12px; padding: 1.5rem; border: 1px solid #334155; }
    .summary-card h3 { color: #94a3b8; font-size: 0.875rem; text-transform: uppercase; letter-spacing: 0.05em; margin-bottom: 0.5rem; }
    .summary-card .value { font-size: 2rem; font-weight: bold; }
    .status-badge { display: inline-block; padding: 0.5rem 1rem; border-radius: 6px; font-weight: 600; font-size: 0.875rem; }
    .status-success { background: ${statusColor}; color: white; }
    .test-cases { margin-top: 2rem; }
    .test-case { background: #1e293b; border-radius: 12px; padding: 1.5rem; margin-bottom: 1.5rem; border: 1px solid #334155; }
    .test-case-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 1rem; }
    .test-case-title { font-size: 1.25rem; font-weight: 600; }
    .test-case-status { padding: 0.25rem 0.75rem; border-radius: 6px; font-size: 0.875rem; font-weight: 600; }
    .status-SUCCESS { background: #10b981; color: white; }
    .status-FAILED { background: #ef4444; color: white; }
    .status-RUNNING { background: #f59e0b; color: white; }
    .test-case-meta { display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 1rem; margin-top: 1rem; }
    .meta-item { color: #94a3b8; font-size: 0.875rem; }
    .meta-item strong { color: #e2e8f0; display: block; margin-bottom: 0.25rem; }
    .error-box { background: #7f1d1d; border: 1px solid #991b1b; border-radius: 8px; padding: 1rem; margin-top: 1rem; }
    .error-box pre { color: #fecaca; font-size: 0.875rem; white-space: pre-wrap; font-family: 'Courier New', monospace; }
    .timestamp { color: #64748b; font-size: 0.875rem; text-align: center; margin-top: 3rem; }
  </style>
</head>
<body>
  <div class="container">
    <div class="header">
      <h1>🧪 Test Execution Report</h1>
      <p>Node Watcher Automated Tests</p>
      <div class="labels">
        <span><strong>Network:</strong> ${encodeURIComponent(GLOBAL_NETWORK)}</span>
        <span><strong>Node ID:</strong> ${encodeURIComponent(GLOBAL_NODE_ID)}</span>
      </div>
    </div>
    <div class="summary">
      <div class="summary-card">
        <h3>Status</h3>
        <div class="value"><span class="status-badge status-success">${statusText}</span></div>
      </div>
      <div class="summary-card">
        <h3>Pass Rate</h3>
        <div class="value" style="color: ${statusColor};">${passRate}%</div>
      </div>
      <div class="summary-card">
        <h3>Total Tests</h3>
        <div class="value" style="color: #6366f1;">${isRunning ? `${completedCount}/${results.cases.length}` : results.cases.length}</div>
      </div>
      <div class="summary-card">
        <h3>Duration</h3>
        <div class="value" style="color: #8b5cf6;">${totalDuration}s</div>
      </div>
    </div>
    <div class="test-cases">
      ${results.cases.map((testCase, index) => {
        const isTestRunning = testCase.endTime === null || testCase.status === 'RUNNING';
        const displayStatus = isTestRunning ? 'RUNNING' : testCase.status;
        const currentDuration = isTestRunning ? Math.round((Date.now() - testCase.startTime) / 1000) : testCase.duration;
        return `
        <div class="test-case">
          <div class="test-case-header">
            <div class="test-case-title">Test Case ${index + 1}: ${encodeURIComponent(testCase.name)}</div>
            <span class="test-case-status status-${displayStatus}">${displayStatus}</span>
          </div>
          <div class="test-case-meta">
            <div class="meta-item">
              <strong>Duration</strong>
              ${currentDuration}s${isTestRunning ? ' (ongoing)' : ''}
            </div>
            <div class="meta-item">
              <strong>Started</strong>
              ${new Date(testCase.startTime).toLocaleTimeString()}
            </div>
            <div class="meta-item">
              <strong>Ended</strong>
              ${isTestRunning ? 'In progress...' : new Date(testCase.endTime).toLocaleTimeString()}
            </div>
            <div class="meta-item">
              <strong>Blocks Accepted</strong>
              ${typeof testCase.blocksAccepted === 'number' && testCase.blocksAccepted !== null ? testCase.blocksAccepted : 'N/A'}
            </div>
            <div class="meta-item">
              <strong>Sync Speed</strong>
              ${typeof testCase.syncSpeed === 'number' && testCase.syncSpeed !== null ? testCase.syncSpeed + ' blocks/s' : 'N/A'}
            </div>
          </div>
          ${testCase.error ? `
            <div class="error-box">
              <strong style="color: #fca5a5; margin-bottom: 0.5rem; display: block;">❌ Error:</strong>
              <pre>${escapeHtml(String(testCase.error))}</pre>
            </div>
          ` : ''}
        </div>
      `;
      }).join('')}
    </div>
    <div class="timestamp">
      Generated at ${new Date(isRunning ? Date.now() : results.endTime).toLocaleString()}${isRunning ? ' (report refreshes on reload)' : ''}
    </div>
  </div>
</body>
</html>`;
}

async function handleStatus(req, res, queryParams) {
  await log('Received /status request');
  
  try {
    const status = {
      status: 'success',
      node: {
        pid: nodePid,
        running: nodePid !== null,
        startTime: nodeStartTime,
        uptime: getUptime(),
        synced: isSynced,
        syncChecking: syncCheckRunning
      },
      server: {
        serverIp: SERVER_IP,
        dbPath: DB_PATH,
        stopTimeout: NODE_STOP_TIMEOUT / 1000,
        stabilityCheckDelay: NODE_STABILITY_CHECK_DELAY / 1000
      },
      endpoint: '/status'
    };
    
    sendJsonResponse(res, 200, status);
  } catch (error) {
    await log(`Error in /status handler: ${error.message}`);
    sendJsonResponse(res, 500, { 
      status: 'error', 
      message: error.message,
      endpoint: '/status'
    });
  }
}

async function handleNotFound(req, res) {
  sendJsonResponse(res, 404, { 
    status: 'error', 
    message: 'Endpoint not found',
    path: req.url
  });
}

async function handleMethodNotAllowed(req, res) {
  res.writeHead(405, { 
    'Content-Type': 'application/json',
    'Allow': 'GET'
  });
  res.end(JSON.stringify({ 
    status: 'error', 
    message: 'Method not allowed. Only GET requests are supported.',
    method: req.method
  }));
}

// ===================================================================
// REQUEST ROUTING
// ===================================================================

// Request handler with async/await
async function handleRequest(req, res) {
  try {
    // Check if the method is GET
    if (req.method !== 'GET') {
      await handleMethodNotAllowed(req, res);
      return;
    }
    
    // Parse URL to extract pathname and query parameters
    const parsedUrl = new URL(req.url, `http://${req.headers.host}`);
    const pathname = parsedUrl.pathname;
    const queryParams = parsedUrl.searchParams;
    
    // Route the request based on the pathname
    switch (pathname) {
      case '/getlogs':
        await handleLogs(req, res, queryParams);
        break;
      case '/status':
        await handleStatus(req, res, queryParams);
        break;
      case '/report':
        await handleReport(req, res, queryParams);
        break;
      default:
        await handleNotFound(req, res);
        break;
    }
  } catch (error) {
    await log(`Error handling request: ${error && error.stack ? error.stack : error}`);
    console.error('Error handling request:', error);
    if (!res.headersSent) {
      res.writeHead(500, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ 
        status: 'error', 
        message: 'Internal server error'
      }));
    }
  }
}

// Create the HTTP server
const server = http.createServer((req, res) => {
  handleRequest(req, res);
});

// Start the server with async/await
async function startServer() {
  const { host, port } = parseAddress();
  // Load global labels before starting server
  await log(`Loaded global labels: network='${GLOBAL_NETWORK}', node_id='${GLOBAL_NODE_ID}'`);
  return new Promise((resolve, reject) => {
    server.listen(port, host, async () => {
      await log(`Server is running on http://${host}:${port}`);
      console.log('Available endpoints:');
      console.log(`  - GET http://${host}:${port}/getlogs?last=N`);
      console.log(`  - GET http://${host}:${port}/status`);
      console.log(`  - GET http://${host}:${port}/report`);
      // Start automated tests
      await log('Starting automated tests...');
      runAllTests().catch(async (error) => {
        await log(`Automated test execution failed: ${error.message}`);
      });
      resolve();
    });
    server.on('error', (err) => {
      if (err.code === 'EADDRINUSE') {
        console.error(`ERROR: Port ${port} is already in use`);
      } else if (err.code === 'EADDRNOTAVAIL') {
        console.error(`ERROR: Address ${host} is not available`);
      } else {
        console.error('ERROR:', err.message);
      }
      reject(err);
    });
  });
}

// Graceful shutdown handler for HTTP server and node process
async function shutdown(signal) {
  await log(`${signal} received, shutting down HTTP server and node process gracefully`);
  
  // Stop the node process if running
  await stopNode();
  
  return new Promise((resolve) => {
    server.close(async () => {
      await log('HTTP server closed');
      resolve();
    });
  });
}


// ===================================================================
// NODE LOG ROTATION
// ===================================================================

// Rotate node log file: if exists, move to _N where N is next available
async function rotateNodeLog() {
  try {
    // Check if log file exists
    await fs.access(NODE_LOG_FILE);
  } catch (e) {
    await log(`Node log file not found, nothing to rotate: ${NODE_LOG_FILE}`);
    return;
  }
  // Find next available suffix N, insert before extension
  const parsed = path.parse(NODE_LOG_FILE);
  let n = 1;
  let nextLog;
  while (true) {
    nextLog = path.join(parsed.dir, `${parsed.name}_${n}${parsed.ext}`);
    try {
      await fs.access(nextLog);
      n++;
    } catch (e) {
      break;
    }
  }
  // Move the log file
  await fs.rename(NODE_LOG_FILE, nextLog);
  await log(`Node log rotated: ${NODE_LOG_FILE} -> ${nextLog}`);
}

// ===================================================================
// NODE PROCESS MANAGEMENT
// ===================================================================

// Function to start the node process if not already running
async function startNode() {
  try {
    await log('Starting node process check...');
    
    // Check if node process is already running
    const { stdout } = await execPromise('pgrep -f "ton-node" || true');
    const pids = stdout.trim().split(/\s+/).filter(pid => pid);
    
    if (pids.length > 0) {
      // Found existing node process(es)
      const pid = pids[0]; // Use the first one
      await log(`Found existing node process with PID: ${pid}`);
      
      // Save PID to global variable
      nodePid = parseInt(pid, 10);
      nodeStartTime = Date.now();
      await log(`PID ${pid} saved to global variable`);
      return;
    }

    // No existing node process found
    await log('No existing node process found, starting new one...');
    
    
    
    // No existing node process, start a new one
    const nodeProcess = spawn('ton-node', NODE_RUN_ARGS, {
      detached: true
    });
    nodeStartTime = Date.now();
    const pid = nodeProcess.pid;
    // Set nodePid BEFORE exit handler to avoid race condition
    nodePid = pid;
    nodeProcess.on('exit', (code, signal) => {
      log(`Node process exited with code ${code}, signal ${signal}`);
      nodePid = null;
      nodeStartTime = null;
      syncCheckRunning = false;
      isSynced = false;
    });
    // Detach the process so it continues running independently
    nodeProcess.unref();
    await log(`Started new node process with PID: ${pid}`);
    await log(`Waiting ${NODE_STABILITY_CHECK_DELAY / 1000} seconds to verify process stability...`);
    await new Promise(resolve => setTimeout(resolve, NODE_STABILITY_CHECK_DELAY));
    try {
      process.kill(pid, 0);
      await log(`Process ${pid} is still running after ${NODE_STABILITY_CHECK_DELAY / 1000} seconds`);
      await log(`PID ${pid} confirmed in global variable`);
    } catch (error) {
      await log(`ERROR: Process ${pid} is no longer running after ${NODE_STABILITY_CHECK_DELAY / 1000} seconds`);
      await log('Node process failed to start or crashed immediately');
      try {
        const { stdout: nodeLogs } = await execPromise(`timeout 5 tail -n 100 "${NODE_LOG_FILE}"`);
        if (nodeLogs.trim()) {
          await log('=== Last 100 lines of node log ===');
          const logLines = nodeLogs.trim().split('\n');
          for (const line of logLines) {
            await log(`[NODE] ${line}`);
          }
          await log('=== End of node log ===');
        } else {
          await log('Node log file is empty');
        }
      } catch (logError) {
        await log(`Could not read node logs: ${logError.message}`);
      }
      nodePid = null;
      nodeStartTime = null;
      throw new Error(`Node process failed to start - exited within ${NODE_STABILITY_CHECK_DELAY / 1000} seconds`);
    }
    
  } catch (error) {
    await log(`Error in startNode: ${error.message}`);
    throw error;
  }
}


// Wait for node to sync by polling the log
async function waitForSync() {
  if (!nodePid) {
    syncCheckRunning = false;
    isSynced = false;
    await log('WARNING: No node process running, cannot wait for sync');
    return;
  }

  syncCheckRunning = true;
  isSynced = false;
  await log(`Waiting for node to sync (log polling, timeout ${Math.round(SYNC_WAIT_TIMEOUT / 3600000)}h)...`);

  try {
    let prevMasterAge = null;
    let prevShardAge = null;
    let lastProgressAt = Date.now();
    while (true) {
      // Read last "Applied master block" and "Applied block" lines
      let masterLine = null, shardLine = null;
      try {
        const { stdout: masterStdout } = await execPromise(`grep 'Applied master block' "${NODE_LOG_FILE}" | tail -n 1`);
        masterLine = masterStdout.trim();
      } catch {}
      try {
        const { stdout: shardStdout } = await execPromise(`grep 'Applied block' "${NODE_LOG_FILE}" | tail -n 1`);
        shardLine = shardStdout.trim();
      } catch {}

      // Parse "Ns old" from both lines
      function parseAge(line) {
        if (!line) return null;
        const m = line.match(/(\d+)s old/);
        return m ? parseInt(m[1], 10) : null;
      }
      const masterAge = parseAge(masterLine);
      const shardAge = parseAge(shardLine);

      // If both values < 10, consider sync complete
      if (masterAge !== null && shardAge !== null && masterAge < 10 && shardAge < 10) {
        isSynced = true;
        await log(`Sync complete: masterAge=${masterAge}, shardAge=${shardAge}`);
        return;
      }

      // Check progress: if at least one value decreased, reset lastProgressAt
      if (
        (prevMasterAge !== null && masterAge !== null && masterAge < prevMasterAge) ||
        (prevShardAge !== null && shardAge !== null && shardAge < prevShardAge)
      ) {
        lastProgressAt = Date.now();
      }

      // Save current values for the next iteration
      prevMasterAge = masterAge;
      prevShardAge = shardAge;

      // Timeout only if there is no progress
      const elapsed = Date.now() - lastProgressAt;
      if (elapsed >= SYNC_WAIT_TIMEOUT) {
        const timeoutSeconds = Math.round(elapsed / 1000);
        const timeoutError = new SyncTimeoutError(`Sync timeout exceeded after ${timeoutSeconds} seconds (no progress)`);
        await log(`ERROR: ${timeoutError.message}`);
        await stopNode();
        throw timeoutError;
      }

      await new Promise(r => setTimeout(r, 2000)); // Wait before next attempt
    }
  } finally {
    syncCheckRunning = false;
  }
}

// ===================================================================
// TEST CASES
// ===================================================================

// Test Case 1: Stop node, wipe DB, start node, wait for sync
async function testCase1() {
  await log('========================================');
  await log('=== TEST CASE 1 START ===');
  await log('=== Stop -> Wipe DB -> Start -> Wait for Sync ===');
  await log('========================================');
  const caseStartTime = Date.now();
  let caseEndTime = null;
  let caseDuration = null;
  let blocksAccepted = null, syncSpeed = null;
  let error = null;

  try {
    if(nodePid) {
      await stopNode();
      await log('Test Case 1: Node stopped');
    }
    // Rotate node log before starting node
    await rotateNodeLog();
    await cleanDb();
    await log('Test Case 1: Database wiped');
    await startNode();
    await log('Test Case 1: Node started, waiting for sync...');
    await waitForSync();
    caseEndTime = Date.now();
    caseDuration = Math.round((caseEndTime - caseStartTime) / 1000);
    try {
      const logStats = await analyzeSyncLog(caseDuration);
      blocksAccepted = logStats.blocksAccepted;
      syncSpeed = logStats.syncSpeed;
      await log(`Test Case 1: Blocks accepted=${blocksAccepted}, speed=${syncSpeed} blocks/s`);
    } catch (e) {
      await log(`Test Case 1: Failed to analyze log: ${e.message}`);
    }
    await log('========================================');
    await log(`=== TEST CASE 1 STOP ===`);
    await log(`=== Duration: ${caseDuration} seconds ===`);
    await log(`=== Status: SUCCESS ===`);
    await log('========================================');
  } catch (err) {
    error = err;
    if (err instanceof SyncTimeoutError) {
      await log(`Test Case 1: Timeout exceeded: ${err.message}`);
    }
    caseEndTime = Date.now();
    caseDuration = Math.round((caseEndTime - caseStartTime) / 1000);
    await log('========================================');
    await log(`=== TEST CASE 1 STOP ===`);
    await log(`=== Status: FAILED - ${err.message}`);
    await log('========================================');
  }

  return {
    name: 'Stop -> Wipe DB -> Start -> Wait for Sync',
    startTime: caseStartTime,
    endTime: caseEndTime,
    duration: caseDuration,
    status: error ? 'FAILED' : 'SUCCESS',
    error: error ? error.message : null,
    blocksAccepted,
    syncSpeed
  };
}

// Test Case 2: Stop node, start node, wait for sync
async function testCase2() {
  await log('========================================');
  await log('=== TEST CASE 2 START ===');
  await log('=== Stop -> Start -> Wait for Sync ===');
  await log('========================================');
  const caseStartTime = Date.now();
  let caseEndTime = null;
  let caseDuration = null;
  let blocksAccepted = null, syncSpeed = null;
  let error = null;

  try {
    if (nodePid) {
      await stopNode();
      await log('Test Case 2: Node stopped');
    }
    // Rotate node log before starting node
    await rotateNodeLog();
    await startNode();
    await log('Test Case 2: Node started, waiting for sync...');
    await waitForSync();
    caseEndTime = Date.now();
    caseDuration = Math.round((caseEndTime - caseStartTime) / 1000);
    try {
      const logStats = await analyzeSyncLog(caseDuration);
      blocksAccepted = logStats.blocksAccepted;
      syncSpeed = logStats.syncSpeed;
      await log(`Test Case 2: Blocks accepted=${blocksAccepted}, speed=${syncSpeed} blocks/s`);
    } catch (e) {
      await log(`Test Case 2: Failed to analyze log: ${e.message}`);
    }
    await log('========================================');
    await log(`=== TEST CASE 2 STOP ===`);
    await log(`=== Duration: ${caseDuration} seconds ===`);
    await log(`=== Status: SUCCESS ===`);
    await log('========================================');
  } catch (err) {
    error = err;
    if (err instanceof SyncTimeoutError) {
      await log(`Test Case 2: Timeout exceeded: ${err.message}`);
    }
    caseEndTime = Date.now();
    caseDuration = Math.round((caseEndTime - caseStartTime) / 1000);
    await log('========================================');
    await log(`=== TEST CASE 2 STOP ===`);
    await log(`=== Status: FAILED - ${err.message}`);
    await log('========================================');
  }

  // Stop node after test case completes
  await stopNode();
  await log('Test Case 2: Node stopped');

  return {
    name: 'Stop -> Start -> Wait for Sync',
    startTime: caseStartTime,
    endTime: caseEndTime,
    duration: caseDuration,
    status: error ? 'FAILED' : 'SUCCESS',
    error: error ? error.message : null,
    blocksAccepted,
    syncSpeed
  };
}

// Run all test cases in sequence (one-by-one)
async function runAllTests() {
  await log('====== Starting All Test Cases (Sequential Execution) ======');
  const overallStartTime = Date.now();
  
  // Reset test results for new run
  testResults = {
    startTime: overallStartTime,
    endTime: null,
    cases: []
  };
  
  try {
    // Add placeholder for Case 1
    testResults.cases.push({
      name: 'Stop -> Wipe DB -> Start -> Wait for Sync',
      startTime: Date.now(),
      endTime: null,
      duration: 0,
      status: 'RUNNING',
      error: null
    });
    
    // Execute Case 1
    const result1 = await testCase1();
    testResults.cases[0] = result1; // Update with actual result
    await log('>>> Proceeding to Test Case 2...');
    
    // Add placeholder for Case 2
    testResults.cases.push({
      name: 'Stop -> Start -> Wait for Sync',
      startTime: Date.now(),
      endTime: null,
      duration: 0,
      status: 'RUNNING',
      error: null
    });
    
    // Execute Case 2
    const result2 = await testCase2();
    testResults.cases[1] = result2; // Update with actual result
    
    const totalTime = Math.round((Date.now() - overallStartTime) / 1000);
    testResults.endTime = Date.now();
    
    await log(`====== All Test Cases Completed in ${totalTime} seconds ======`);
    await log(`Report available at: /report`);
    // Send report to Slack
    try {
      await sendSlackReport(testResults);
      await log('Slack report sent');
    } catch (e) {
      await log('Failed to send Slack report: ' + e.message);
    }
  } catch (error) {
    testResults.endTime = Date.now();
    await log(`====== Test execution failed: ${error.message} ======`);
  }
  // Only exit if all tests succeeded
  const allPassed = testResults.cases.every(c => c.status === 'SUCCESS');
  if (allPassed) {
    await log('All tests complete, exiting...');
    process.exit(0);
  } else {
    await log('All tests complete, but some tests failed. Server will remain running for investigation.');
    // Ensure all node processes are stopped
    await stopAllNodeProcesses();
    // Do not exit, keep server running
  }
}

// Builds and sends a report to Slack
async function sendSlackReport(results) {
  // Short summary message
  const case1 = results.cases[0];
  const case2 = results.cases[1];
  const msg = `Node: ${GLOBAL_NODE_ID}\nNetwork: ${GLOBAL_NETWORK}\nCase 1: ${case1.status === 'SUCCESS' ? 'success' : 'fail'} in ${case1.duration} seconds\nCase 2: ${case2.status === 'SUCCESS' ? 'success' : 'fail'} in ${case2.duration} seconds`;

  let fileSent = false;
  let fileError = null;
  // Try to send HTML report as file if both token and channel are set
  if (SLACK_BOT_TOKEN && SLACK_CHANNEL_ID) {
    try {
      if (!FormData) FormData = require('form-data');
      const html = generateHtmlReport(results);
      const tmpPath = `/tmp/node_report_${Date.now()}.html`;
      await fs.writeFile(tmpPath, html, 'utf8');
      await uploadFileToSlack(tmpPath, 'Node Test Report', msg);
      await fs.unlink(tmpPath).catch(() => {});
      fileSent = true;
    } catch (e) {
      fileError = e;
      await log('Slack file upload failed: ' + e.message);
    }
  }

  // If file not sent, send message to channel (webhook or chat.postMessage)
  if (!fileSent) {
    // Prefer webhook if set
    if (SLACK_WEBHOOK_URL) {
      const payload = { text: msg };
      await postToSlack(payload);
    } else if (SLACK_BOT_TOKEN && SLACK_CHANNEL_ID) {
      // Fallback: use chat.postMessage
      await sendSlackTextMessage(SLACK_CHANNEL_ID, msg);
    } else {
      await log('No Slack credentials for sending message');
    }
  }
}
// Send a plain text message to a Slack channel using chat.postMessage
async function sendSlackTextMessage(channel, text) {
  const payload = JSON.stringify({ channel, text });
  const options = {
    method: 'POST',
    hostname: 'slack.com',
    path: '/api/chat.postMessage',
    headers: {
      'Authorization': `Bearer ${SLACK_BOT_TOKEN}`,
      'Content-Type': 'application/json',
      'Content-Length': Buffer.byteLength(payload)
    }
  };
  await new Promise((resolve, reject) => {
    const req = https.request(options, (res) => {
      let data = '';
      res.on('data', chunk => { data += chunk; });
      res.on('end', async () => {
        try {
          const json = JSON.parse(data);
          await log(`Slack chat.postMessage response: ${data}`);
          if (json.ok) resolve();
          else reject(new Error('Slack chat.postMessage error: ' + (json.error || data)));
        } catch (e) { reject(e); }
      });
    });
    req.on('error', reject);
    req.write(payload);
    req.end();
  });
}

// Upload file to Slack using files.upload API
async function uploadFileToSlack(filePath, title, initialComment) {
  // Step 1: Get upload URL and file_id
  const stat = require('fs').statSync(filePath);
  const fileSize = stat.size;
  const fileName = 'report.html';
  const getUrlForm = new (require('form-data'))();
  getUrlForm.append('filename', fileName);
  getUrlForm.append('length', fileSize.toString());
  const getUrlOptions = {
    method: 'POST',
    headers: {
      'Authorization': `Bearer ${SLACK_BOT_TOKEN}`,
      ...getUrlForm.getHeaders()
    }
  };
  const uploadUrlResp = await new Promise((resolve, reject) => {
    const req = https.request('https://slack.com/api/files.getUploadURLExternal', getUrlOptions, (res) => {
      let data = '';
      res.on('data', chunk => { data += chunk; });
      res.on('end', () => {
        try {
          const json = JSON.parse(data);
          if (json.ok && json.upload_url && json.file_id) resolve(json);
          else reject(new Error('Slack getUploadURLExternal error: ' + (json.error || data)));
        } catch (e) { reject(e); }
      });
    });
    req.on('error', reject);
    getUrlForm.pipe(req);
  });
  const { upload_url, file_id } = uploadUrlResp;

  // Step 2: Upload file binary to upload_url
  const fileBuffer = require('fs').readFileSync(filePath);
  await new Promise((resolve, reject) => {
    const url = new URL(upload_url);
    const options = {
      method: 'POST',
      hostname: url.hostname,
      path: url.pathname + url.search,
      headers: {
        'Content-Type': 'application/octet-stream',
        'Content-Length': fileBuffer.length
      }
    };
    const req = https.request(options, (res) => {
      let data = '';
      res.on('data', chunk => { data += chunk; });
      res.on('end', async () => {
        if (res.statusCode >= 200 && res.statusCode < 300) resolve();
        else reject(new Error('Slack upload_url HTTP error: ' + res.statusCode + ' ' + data));
      });
    });
    req.on('error', reject);
    req.write(fileBuffer);
    req.end();
  });

  // Step 3: Complete upload and share in channel
  // Find a default channel from env or fallback
  const channel = SLACK_CHANNEL_ID || null;
  if (!channel) {
    await log('No SLACK_CHANNEL_ID  set, file will not be shared in a channel');
    return;
  }
  const completeForm = new (require('form-data'))();
  completeForm.append('files', JSON.stringify([{ id: file_id, title: title || fileName }]));
  completeForm.append('channel_id', channel);
  if (initialComment) completeForm.append('initial_comment', initialComment);
  const completeOptions = {
    method: 'POST',
    headers: {
      'Authorization': `Bearer ${SLACK_BOT_TOKEN}`,
      ...completeForm.getHeaders()
    }
  };
  await new Promise((resolve, reject) => {
    const req = https.request('https://slack.com/api/files.completeUploadExternal', completeOptions, (res) => {
      let data = '';
      res.on('data', chunk => { data += chunk; });
      res.on('end', () => {
        try {
          const json = JSON.parse(data);
          if (json.ok) resolve();
          else reject(new Error('Slack completeUploadExternal error: ' + (json.error || data)));
        } catch (e) { reject(e); }
      });
    });
    req.on('error', reject);
    completeForm.pipe(req);
  });
}

function postToSlack(payload) {
  return new Promise((resolve, reject) => {
    const url = new URL(SLACK_WEBHOOK_URL);
    const data = JSON.stringify(payload);
    const options = {
      hostname: url.hostname,
      path: url.pathname + url.search,
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Content-Length': Buffer.byteLength(data)
      }
    };
    const req = https.request(options, (res) => {
      let body = '';
      res.on('data', (chunk) => { body += chunk; });
      res.on('end', () => {
        if (res.statusCode >= 200 && res.statusCode < 300) resolve();
        else reject(new Error('Slack error: ' + res.statusCode + ' ' + body));
      });
    });
    req.on('error', reject);
    req.write(data);
    req.end();
  });
}

// ===================================================================
// NODE LIFECYCLE FUNCTIONS
// ===================================================================

// Stop node process if it's running
async function stopNode() {
  if (!nodePid) {
    await log('No node process is currently running');
    return;
  }
  
  try {
    const savedPid = nodePid;
    await log(`Attempting to stop node process with PID: ${savedPid}`);
    
    // Stop sync checking and reject waiting promises
    syncCheckRunning = false;
    isSynced = false;
    
    // Reject all waiting promises before clearing
    const waitersToReject = [...syncWaiters];
    syncWaiters = [];
    waitersToReject.forEach(resolve => {
      // Resolve instead of reject to avoid unhandled rejections
      // The waiters will just get unblocked when node stops
      resolve();
    });
    
    await stopProcessWithTimeout(savedPid);
    nodePid = null;
    nodeStartTime = null;
  } catch (error) {
    await log(`Error stopping node process: ${error.message}`);
    nodePid = null;
    nodeStartTime = null;
  }
}

// ===================================================================
// DATABASE MANAGEMENT
// ===================================================================

// Clean the database folder contents (keep the folder itself)
async function cleanDb() {
  try {
    await log(`Cleaning database contents at path: ${DB_PATH}`);
    
    // Check if the db directory exists
    try {
      await fs.access(DB_PATH);
    } catch (err) {
      await log(`Database directory does not exist, nothing to clean`);
      return;
    }
    
    // Get all items in the directory
    const items = await fs.readdir(DB_PATH);
    
    if (items.length === 0) {
      await log(`Database directory is already empty`);
      return;
    }
    
    await log(`Found ${items.length} items to delete`);
    
    // Delete each item in the directory (but keep the directory itself)
    for (const item of items) {
      const itemPath = path.join(DB_PATH, item);
      await fs.rm(itemPath, { recursive: true, force: true });
    }
    
    await log(`Database directory contents cleaned successfully`);
  } catch (error) {
    await log(`Error cleaning database: ${error.message}`);
  }
}

// ===================================================================
// SIGNAL HANDLERS & STARTUP
// ===================================================================

// Signal handlers for graceful shutdown
process.on('SIGTERM', async () => {
  await shutdown('SIGTERM');
  process.exit(0);
});

process.on('SIGINT', async () => {
  await shutdown('SIGINT');
  process.exit(0);
});

// Handle uncaught errors
process.on('uncaughtException', async (error) => {
  await log(`Uncaught exception: ${error && error.stack ? error.stack : error}`);
  console.error('Uncaught exception:', error);
  await shutdown('UNCAUGHT_EXCEPTION');
  process.exit(0);
});

process.on('unhandledRejection', async (reason, promise) => {
  await log(`Unhandled rejection at: ${promise}, reason: ${reason}`);
  console.error('Unhandled rejection:', reason);
});

// Start the server
startServer().catch(async (err) => {
  await log(`Failed to start server: ${err && err.stack ? err.stack : err}`);
  process.exit(0);
});
