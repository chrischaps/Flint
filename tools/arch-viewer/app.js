// app.js — Flint Architecture Explorer

const TIER_COLORS = {
  0: '#4ade80', 1: '#60a5fa', 2: '#a78bfa',
  3: '#fbbf24', 4: '#f87171', 5: '#f87171',
  6: '#f87171', 7: '#f87171',
};

const TIER_BG = {
  0: '#1e3a2e', 1: '#1e2a3a', 2: '#2a1e3a',
  3: '#3a2a1e', 4: '#3a1e1e', 5: '#3a1e1e',
  6: '#3a1e1e', 7: '#3a1e1e',
};

const TIER_NAMES = {
  0: 'Core', 1: 'ECS', 2: 'Scene',
  3: 'Systems', 4: 'Integration', 5: 'Integration',
  6: 'Integration', 7: 'Aggregators',
};

let cy;
let archData = null;

// Data lookup registry — avoids inline onclick with JSON.stringify
const _dataRegistry = {};
let _dataId = 0;

function clearRegistry() {
  Object.keys(_dataRegistry).forEach(function(k) { delete _dataRegistry[k]; });
  _dataId = 0;
}

function registerData(obj) {
  const id = '_d' + (_dataId++);
  _dataRegistry[id] = obj;
  return id;
}

function lookupData(id) {
  return _dataRegistry[id];
}

// Escape HTML to prevent XSS from data values
function escapeHtml(str) {
  const div = document.createElement('div');
  div.textContent = str;
  return div.innerHTML;
}

async function init() {
  try {
    const resp = await fetch('arch-data.json');
    if (!resp.ok) throw new Error('HTTP ' + resp.status);
    archData = await resp.json();
  } catch (e) {
    document.getElementById('detail-content').innerHTML =
      '<p style="color:#f87171;text-align:center;margin-top:40%">Failed to load arch-data.json.<br>Run flint-arch-analyzer first.</p>';
    return;
  }

  buildGraph();
  buildTierFilters();
  setupSearch();
  setupLayoutButtons();
  setupTools();
  setupDetailDelegation();
  updateFooter();
}

function buildGraph() {
  const elements = [];

  // Crate nodes
  for (const crate of archData.crates) {
    const dependentCount = archData.edges.filter(e => e.to === crate.name).length;
    elements.push({
      group: 'nodes',
      data: {
        id: crate.name,
        label: crate.name.replace('flint-', ''),
        tier: crate.tier,
        lines: crate.lines,
        dependentCount,
        type: 'crate',
        crateData: crate,
      },
    });
  }

  // Edges
  for (const edge of archData.edges) {
    elements.push({
      group: 'edges',
      data: {
        id: `${edge.from}->${edge.to}`,
        source: edge.from,
        target: edge.to,
      },
    });
  }

  cy = cytoscape({
    container: document.getElementById('graph-container'),
    elements,
    style: [
      {
        selector: 'node[type="crate"]',
        style: {
          'label': 'data(label)',
          'text-valign': 'center',
          'text-halign': 'center',
          'font-size': '11px',
          'color': ele => TIER_COLORS[ele.data('tier')] || '#ccc',
          'background-color': ele => TIER_BG[ele.data('tier')] || '#252540',
          'border-width': ele => Math.min(1 + ele.data('dependentCount') * 0.5, 4),
          'border-color': ele => TIER_COLORS[ele.data('tier')] || '#ccc',
          'shape': 'roundrectangle',
          'width': 'label',
          'height': 30,
          'padding': '12px',
          'text-wrap': 'none',
        },
      },
      {
        selector: 'node[type="module"]',
        style: {
          'label': 'data(label)',
          'text-valign': 'center',
          'text-halign': 'center',
          'font-size': '9px',
          'color': '#ccc',
          'background-color': '#252540',
          'border-width': 1,
          'border-color': '#444',
          'shape': 'roundrectangle',
          'width': 'label',
          'height': 22,
          'padding': '8px',
        },
      },
      {
        selector: ':parent',
        style: {
          'background-color': ele => {
            const color = TIER_COLORS[ele.data('tier')] || '#ccc';
            return color + '08';
          },
          'border-style': 'dashed',
          'border-width': 1,
          'border-color': ele => (TIER_COLORS[ele.data('tier')] || '#ccc') + '40',
          'text-valign': 'top',
          'text-halign': 'center',
          'font-size': '10px',
          'padding': '12px',
        },
      },
      {
        selector: 'edge',
        style: {
          'width': 1.5,
          'line-color': '#ffffff15',
          'target-arrow-color': '#ffffff30',
          'target-arrow-shape': 'triangle',
          'curve-style': 'bezier',
          'arrow-scale': 0.8,
        },
      },
      {
        selector: 'edge.highlighted',
        style: {
          'line-color': '#5865F2',
          'target-arrow-color': '#5865F2',
          'width': 2.5,
          'z-index': 10,
        },
      },
      {
        selector: 'node.highlighted',
        style: {
          'border-width': 3,
          'border-color': '#5865F2',
          'z-index': 10,
        },
      },
      {
        selector: 'node.dimmed, edge.dimmed',
        style: { 'opacity': 0.15 },
      },
      {
        selector: 'node.search-match',
        style: {
          'border-width': 3,
          'border-color': '#fff',
          'z-index': 10,
        },
      },
    ],
    layout: { name: 'dagre', rankDir: 'TB', spacingFactor: 1.2, nodeSep: 60, rankSep: 80 },
    wheelSensitivity: 0.3,
  });

  // Click handlers
  cy.on('tap', 'node[type="crate"]', onCrateClick);
  cy.on('tap', 'node[type="module"]', onModuleClick);
  cy.on('tap', 'edge', onEdgeClick);
  cy.on('tap', function(e) {
    if (e.target === cy) { clearSelection(); }
  });
}

function onCrateClick(e) {
  const node = e.target;
  const crateData = node.data('crateData');

  if (node.isParent()) {
    // Collapse: remove child module nodes
    collapseCrate(node);
  } else {
    // Expand: add module nodes as children
    expandCrate(node, crateData);
  }

  showCrateDetail(crateData);
}

function expandCrate(node, crateData) {
  if (!crateData.modules || crateData.modules.length === 0) return;

  const flatModules = flattenModules(crateData.modules, crateData.name);
  for (const mod of flatModules) {
    cy.add({
      group: 'nodes',
      data: {
        id: mod.id,
        label: mod.name,
        parent: crateData.name,
        type: 'module',
        moduleData: mod.data,
        parentCrate: crateData.name,
      },
    });
  }

  // Re-run layout just for the expanded children
  cy.layout({
    name: 'grid',
    fit: false,
    boundingBox: node.boundingBox(),
    rows: Math.ceil(Math.sqrt(flatModules.length)),
  }).run();
}

function collapseCrate(node) {
  const children = node.children();
  children.remove();
}

function flattenModules(modules, crateId, prefix) {
  prefix = prefix || '';
  const result = [];
  for (const mod of modules) {
    const id = crateId + '::' + prefix + mod.name;
    result.push({ id: id, name: mod.name, data: mod });
    if (mod.children && mod.children.length > 0) {
      result.push.apply(result, flattenModules(mod.children, crateId, prefix + mod.name + '::'));
    }
  }
  return result;
}

function onModuleClick(e) {
  const node = e.target;
  showModuleDetail(node.data('moduleData'), node.data('parentCrate'));
}

function onEdgeClick(e) {
  const edge = e.target;
  showEdgeDetail(edge.data('source'), edge.data('target'));
}

function clearSelection() {
  cy.elements().removeClass('highlighted dimmed');
  document.getElementById('detail-content').innerHTML =
    '<p class="detail-placeholder">Click a node or edge to see details</p>';
}

// ---- Event Delegation for Detail Panel ----
// Instead of inline onclick handlers with JSON.stringify, we use
// data attributes and a single delegated click handler.

function setupDetailDelegation() {
  document.getElementById('detail-content').addEventListener('click', function(e) {
    // Walk up from the clicked element to find one with a data-action
    let target = e.target;
    while (target && target !== this) {
      const action = target.dataset.action;
      if (action === 'navigate') {
        navigateTo(target.dataset.nodeId);
        return;
      }
      if (action === 'show-module') {
        const data = lookupData(target.dataset.ref);
        if (data) showModuleDetail(data.mod, data.crateName);
        return;
      }
      if (action === 'show-item') {
        const data = lookupData(target.dataset.ref);
        if (data) showItemDetail(data);
        return;
      }
      target = target.parentElement;
    }
  });
}

// ---- Detail Panel Renderers ----

function showCrateDetail(crate) {
  clearRegistry();
  const tierColor = TIER_COLORS[crate.tier] || '#ccc';
  let html = '';
  html += '<div class="detail-label">Crate</div>';
  html += '<div class="detail-name" style="color:' + tierColor + '">' + escapeHtml(crate.name) + '</div>';
  html += '<div class="detail-path">' + escapeHtml(crate.path) + '</div>';
  html += '<div class="detail-stat">' + crate.lines.toLocaleString() + ' lines &middot; Tier ' + crate.tier + '</div>';

  if (crate.description) {
    html += '<div class="detail-stat" style="margin-top:4px">' + escapeHtml(crate.description) + '</div>';
  }

  if (crate.internal_deps.length > 0) {
    html += '<div class="detail-label">Internal Dependencies</div>';
    for (const dep of crate.internal_deps) {
      const depCrate = archData.crates.find(function(c) { return c.name === dep; });
      const depColor = TIER_COLORS[depCrate ? depCrate.tier : undefined] || '#ccc';
      html += '<div class="dep-link" style="color:' + depColor + '" data-action="navigate" data-node-id="' + escapeHtml(dep) + '">&rarr; ' + escapeHtml(dep) + '</div>';
    }
  }

  if (crate.external_deps.length > 0) {
    html += '<div class="detail-label">External Dependencies</div>';
    html += '<div class="detail-stat">' + crate.external_deps.map(escapeHtml).join(', ') + '</div>';
  }

  if (crate.modules.length > 0) {
    html += '<div class="detail-label">Modules</div>';
    for (const mod of crate.modules) {
      const ref = registerData({ mod: mod, crateName: crate.name });
      html += '<div class="item-card" data-action="show-module" data-ref="' + ref + '">';
      html += '<div class="item-name">' + escapeHtml(mod.name) + '</div>';
      html += '<div class="detail-stat">' + mod.lines + ' lines</div>';
      html += '</div>';
    }
  }

  document.getElementById('detail-content').innerHTML = html;
}

function showModuleDetail(mod, crateName) {
  clearRegistry();
  var depCrate = archData.crates.find(function(c) { return c.name === crateName; });
  const tierColor = TIER_COLORS[depCrate ? depCrate.tier : undefined] || '#ccc';
  let html = '';
  html += '<div class="detail-label">Module</div>';
  html += '<div class="detail-name" style="color:' + tierColor + '">' + escapeHtml(mod.name) + '</div>';
  html += '<div class="detail-path">' + escapeHtml(mod.path) + '</div>';
  html += '<div class="detail-stat">' + mod.lines + ' lines</div>';

  if (mod.public_items && mod.public_items.length > 0) {
    html += '<div class="detail-label">Public API</div>';
    for (const item of mod.public_items) {
      const ref = registerData(item);
      html += '<div class="item-card ' + escapeHtml(item.kind) + '" data-action="show-item" data-ref="' + ref + '">';
      html += '<div class="item-kind ' + escapeHtml(item.kind) + '">' + escapeHtml(item.kind) + '</div>';
      html += '<div class="item-name">' + escapeHtml(item.name) + '</div>';
      html += '</div>';
    }
  }

  if (mod.children && mod.children.length > 0) {
    html += '<div class="detail-label">Submodules</div>';
    for (const child of mod.children) {
      const ref = registerData({ mod: child, crateName: crateName });
      html += '<div class="item-card" data-action="show-module" data-ref="' + ref + '">';
      html += '<div class="item-name">' + escapeHtml(child.name) + '</div>';
      html += '<div class="detail-stat">' + child.lines + ' lines</div>';
      html += '</div>';
    }
  }

  document.getElementById('detail-content').innerHTML = html;
}

function showItemDetail(item) {
  clearRegistry();
  let html = '';
  html += '<div class="detail-label">' + escapeHtml(item.kind) + '</div>';
  html += '<div class="detail-name">' + escapeHtml(item.name) + '</div>';

  if (item.members && item.members.length > 0) {
    var label;
    if (item.kind === 'fn') label = 'Signature';
    else if (item.kind === 'trait') label = 'Methods';
    else if (item.kind === 'enum') label = 'Variants';
    else label = 'Fields';

    html += '<div class="detail-label">' + label + '</div>';
    for (const member of item.members) {
      html += '<div class="member-row">';
      html += '<span class="member-name">' + escapeHtml(member.name) + ':</span> ';
      html += '<span class="member-type">' + escapeHtml(member.type) + '</span>';
      html += '</div>';
    }
  }

  document.getElementById('detail-content').innerHTML = html;
}

function showEdgeDetail(source, target) {
  clearRegistry();
  const srcCrate = archData.crates.find(function(c) { return c.name === source; });
  const tgtCrate = archData.crates.find(function(c) { return c.name === target; });
  const srcColor = TIER_COLORS[srcCrate ? srcCrate.tier : undefined] || '#ccc';
  const tgtColor = TIER_COLORS[tgtCrate ? tgtCrate.tier : undefined] || '#ccc';

  let html = '';
  html += '<div class="detail-label">Dependency</div>';
  html += '<div class="dep-link" style="color:' + srcColor + '" data-action="navigate" data-node-id="' + escapeHtml(source) + '">' + escapeHtml(source) + '</div>';
  html += '<div style="color:var(--text-dim);margin:4px 0">depends on</div>';
  html += '<div class="dep-link" style="color:' + tgtColor + '" data-action="navigate" data-node-id="' + escapeHtml(target) + '">' + escapeHtml(target) + '</div>';

  document.getElementById('detail-content').innerHTML = html;

  // Highlight the edge
  cy.elements().removeClass('highlighted dimmed');
  const edge = cy.getElementById(source + '->' + target);
  edge.addClass('highlighted');
  cy.getElementById(source).addClass('highlighted');
  cy.getElementById(target).addClass('highlighted');
}

function navigateTo(nodeId) {
  const node = cy.getElementById(nodeId);
  if (node.length > 0) {
    cy.animate({ center: { eles: node }, zoom: cy.zoom() }, { duration: 300 });
    node.addClass('highlighted');
    setTimeout(function() { node.removeClass('highlighted'); }, 1500);
    const crateData = node.data('crateData');
    if (crateData) showCrateDetail(crateData);
  }
}

// ---- Toolbar: Search ----

function setupSearch() {
  const input = document.getElementById('search-input');
  input.addEventListener('input', function() {
    const query = input.value.toLowerCase().trim();
    cy.elements().removeClass('search-match dimmed');

    if (!query) return;

    // Search crate names, module names, and public item names
    const matchingNodes = cy.nodes().filter(function(node) {
      if (node.data('label').toLowerCase().indexOf(query) !== -1) return true;
      const crateData = node.data('crateData');
      if (crateData) {
        return searchInModules(crateData.modules, query);
      }
      return false;
    });

    if (matchingNodes.length > 0) {
      cy.elements().addClass('dimmed');
      matchingNodes.removeClass('dimmed').addClass('search-match');
      matchingNodes.connectedEdges().removeClass('dimmed');
    }
  });
}

function searchInModules(modules, query) {
  for (const mod of modules) {
    if (mod.name.toLowerCase().indexOf(query) !== -1) return true;
    for (const item of (mod.public_items || [])) {
      if (item.name.toLowerCase().indexOf(query) !== -1) return true;
    }
    if (mod.children && searchInModules(mod.children, query)) return true;
  }
  return false;
}

// ---- Toolbar: Layout ----

function setupLayoutButtons() {
  document.querySelectorAll('.layout-btn').forEach(function(btn) {
    btn.addEventListener('click', function() {
      document.querySelectorAll('.layout-btn').forEach(function(b) { b.classList.remove('active'); });
      btn.classList.add('active');
      const layoutName = btn.dataset.layout;

      const layoutOpts = {
        dagre: { name: 'dagre', rankDir: 'TB', spacingFactor: 1.2, nodeSep: 60, rankSep: 80 },
        cose: { name: 'cose', animate: true, animationDuration: 500, nodeRepulsion: 8000, idealEdgeLength: 120 },
        concentric: {
          name: 'concentric',
          concentric: function(node) {
            const maxTier = Math.max.apply(null, archData.crates.map(function(c) { return c.tier; }));
            return maxTier - (node.data('tier') || 0);
          },
          levelWidth: function() { return 1; },
          animate: true,
        },
      };

      cy.layout(layoutOpts[layoutName]).run();
    });
  });
}

// ---- Toolbar: Tier Filters ----

function buildTierFilters() {
  const tiers = [];
  const seen = {};
  for (const c of archData.crates) {
    if (!seen[c.tier]) {
      seen[c.tier] = true;
      tiers.push(c.tier);
    }
  }
  tiers.sort(function(a, b) { return a - b; });

  const container = document.getElementById('tier-filters');

  for (const tier of tiers) {
    const btn = document.createElement('button');
    btn.className = 'tier-toggle';
    btn.style.backgroundColor = TIER_COLORS[tier] + '30';
    btn.style.color = TIER_COLORS[tier];
    btn.textContent = 'T' + tier;
    btn.title = TIER_NAMES[tier] || ('Tier ' + tier);
    btn.dataset.tier = tier;
    btn.addEventListener('click', function() {
      btn.classList.toggle('inactive');
      const hidden = btn.classList.contains('inactive');
      cy.nodes('[tier = ' + tier + ']').forEach(function(node) {
        if (hidden) {
          node.style('display', 'none');
        } else {
          node.style('display', 'element');
        }
      });
    });
    container.appendChild(btn);
  }
}

// ---- Toolbar: Tools ----

function setupTools() {
  setupPathFinder();
  setupMetrics();
  setupDepExplorer();
}

let pathFinderMode = false;
let pathFinderNodes = [];

function setupPathFinder() {
  const btn = document.getElementById('btn-path-finder');
  btn.addEventListener('click', function() {
    pathFinderMode = !pathFinderMode;
    btn.classList.toggle('active', pathFinderMode);
    pathFinderNodes = [];
    cy.elements().removeClass('highlighted dimmed');

    if (pathFinderMode) {
      // Disable other tool modes
      disableDepExplorer();
      cy.off('tap', 'node[type="crate"]', onCrateClick);
      cy.on('tap', 'node[type="crate"]', onPathFinderClick);
      document.getElementById('detail-content').innerHTML =
        '<div class="path-finder-hint">Click a crate node to start. Then click a second to find the shortest path.</div>';
    } else {
      cy.off('tap', 'node[type="crate"]', onPathFinderClick);
      cy.on('tap', 'node[type="crate"]', onCrateClick);
    }
  });
}

function onPathFinderClick(e) {
  const node = e.target;
  pathFinderNodes.push(node);

  if (pathFinderNodes.length === 1) {
    node.addClass('highlighted');
    document.getElementById('detail-content').innerHTML =
      '<div class="path-finder-hint">Select a second node to find the shortest path.</div>';
  } else if (pathFinderNodes.length === 2) {
    const path = cy.elements().dijkstra(pathFinderNodes[0], function() { return 1; }, true);
    const pathTo = path.pathTo(pathFinderNodes[1]);

    cy.elements().addClass('dimmed');
    pathTo.removeClass('dimmed').addClass('highlighted');

    const names = pathTo.nodes().map(function(n) { return n.data('label'); }).join(' \u2192 ');
    document.getElementById('detail-content').innerHTML =
      '<div class="detail-label">Shortest Path</div>' +
      '<div class="detail-stat">' + names + '</div>' +
      '<div class="detail-stat" style="margin-top:8px">' + pathTo.nodes().length + ' nodes, ' + pathTo.edges().length + ' edges</div>';

    pathFinderNodes = [];
  }
}

let metricsActive = false;

function setupMetrics() {
  const btn = document.getElementById('btn-metrics');
  btn.addEventListener('click', function() {
    metricsActive = !metricsActive;
    btn.classList.toggle('active', metricsActive);

    if (metricsActive) {
      const maxLines = Math.max.apply(null, archData.crates.map(function(c) { return c.lines; }));
      cy.nodes('[type="crate"]').forEach(function(node) {
        const lines = node.data('lines') || 0;
        const scale = 30 + (lines / maxLines) * 60;
        node.style({ 'width': scale + 'px', 'height': scale + 'px', 'font-size': '9px' });
      });
    } else {
      cy.nodes('[type="crate"]').forEach(function(node) {
        node.style({ 'width': 'label', 'height': '30px', 'font-size': '11px' });
      });
    }
  });
}

let depExplorerMode = false;

function setupDepExplorer() {
  const btn = document.getElementById('btn-dep-explorer');
  btn.addEventListener('click', function() {
    depExplorerMode = !depExplorerMode;
    btn.classList.toggle('active', depExplorerMode);
    cy.elements().removeClass('highlighted dimmed');

    if (depExplorerMode) {
      // Disable other tool modes
      disablePathFinder();
      cy.off('tap', 'node[type="crate"]', onCrateClick);
      cy.on('tap', 'node[type="crate"]', onDepExplorerClick);
    } else {
      cy.off('tap', 'node[type="crate"]', onDepExplorerClick);
      cy.on('tap', 'node[type="crate"]', onCrateClick);
    }
  });
}

function disablePathFinder() {
  if (pathFinderMode) {
    pathFinderMode = false;
    pathFinderNodes = [];
    document.getElementById('btn-path-finder').classList.remove('active');
    cy.off('tap', 'node[type="crate"]', onPathFinderClick);
  }
}

function disableDepExplorer() {
  if (depExplorerMode) {
    depExplorerMode = false;
    document.getElementById('btn-dep-explorer').classList.remove('active');
    cy.off('tap', 'node[type="crate"]', onDepExplorerClick);
  }
}

function onDepExplorerClick(e) {
  const node = e.target;
  cy.elements().addClass('dimmed');

  // Upstream: what this depends on (successors in dep graph = outgoing edges)
  const upstream = node.successors();
  // Downstream: what depends on this (predecessors = incoming edges)
  const downstream = node.predecessors();

  const all = upstream.union(downstream).union(node);
  all.removeClass('dimmed').addClass('highlighted');

  const upNames = upstream.nodes().map(function(n) { return n.data('label'); });
  const downNames = downstream.nodes().map(function(n) { return n.data('label'); });

  document.getElementById('detail-content').innerHTML =
    '<div class="detail-label">Dependency Explorer</div>' +
    '<div class="detail-name" style="color:' + TIER_COLORS[node.data('tier')] + '">' + escapeHtml(node.data('label')) + '</div>' +
    '<div class="detail-label">Depends On (' + upNames.length + ')</div>' +
    '<div class="detail-stat">' + (upNames.map(escapeHtml).join(', ') || 'None') + '</div>' +
    '<div class="detail-label">Depended On By (' + downNames.length + ')</div>' +
    '<div class="detail-stat">' + (downNames.map(escapeHtml).join(', ') || 'None') + '</div>';
}

// ---- Footer ----

function updateFooter() {
  const footer = document.getElementById('toolbar-footer');
  const date = new Date(archData.generated_at).toLocaleDateString();
  footer.textContent = archData.crates.length + ' crates \u00B7 ' + archData.edges.length + ' edges\nGenerated: ' + date;
}

// ---- Start ----
init();
