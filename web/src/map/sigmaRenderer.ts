import { createNodeImageProgram } from "@sigma/node-image";
import Graph from "graphology";
import Sigma from "sigma";
import { createNodeCompoundProgram, EdgeArrowProgram, EdgeRectangleProgram, NodeCircleProgram } from "sigma/rendering";
import type { NodeDisplayData } from "sigma/types";
import {
  categoryEdgeLook,
  categoryLook,
  clusterLook,
  edgeLook,
  memoryLook,
  memoryPosition,
  neighbourhood,
  type Look,
  type ViewState,
} from "../domain/map/appearance";
import { convexHull, lassoed, type Point } from "../domain/map/geometry";
import type { GroupNode, MapModel } from "../domain/map/layout";
import { categoryStyle, glyphImage, kindStyle, withAlpha } from "../domain/map/palette";
import { LEVEL_RATIO, levelAt, quantize, zoomMix, type Level, type ZoomMix } from "../domain/map/zoom";
import type { MapRenderer, RendererCallbacks, RendererFactory } from "./renderer";

type NodeKind = "memory" | "category" | "cluster";

/** Label text on the map's background: slate-200 on dark, slate-900 on light. */
function labelColor(dark: boolean): string {
  return dark ? "#e2e8f0" : "#0f172a";
}

/**
 * Sigma adapter: puts the model into a graphology graph and lets the pure
 * look functions (domain/map/appearance.ts) decide every node and edge on
 * each refresh. All the map's logic lives there; this file only wires
 * events, the camera and the category hulls.
 */
export const createSigmaRenderer: RendererFactory = (container, model, initialView, callbacks) =>
  new SigmaMapRenderer(container, model, initialView, callbacks);

const PictogramProgram = createNodeCompoundProgram([
  NodeCircleProgram,
  createNodeImageProgram({
    padding: 0.2,
    size: { mode: "force", value: 64 },
    drawingMode: "color",
    colorAttribute: "pictoColor",
  }),
]);

class SigmaMapRenderer implements MapRenderer {
  private readonly sigma: Sigma;
  private readonly graph = new Graph({ multi: true, type: "directed" });
  private readonly hulls: HTMLCanvasElement;
  private view: ViewState;
  private mix: ZoomMix;
  private level: Level;
  private hovered: string | null = null;
  private lassoMode = false;
  /** The outline being drawn, in viewport pixels; `null` when not lassoing. */
  private lasso: Point[] | null = null;
  private readonly lassoCanvas: HTMLCanvasElement;

  constructor(
    container: HTMLElement,
    private readonly model: MapModel,
    view: ViewState,
    private readonly callbacks: RendererCallbacks,
  ) {
    this.view = view;
    // One category's own map already fills the view with that category: its
    // zoom bands start one level in, so it opens split into its clusters.
    this.bandScale = model.scope !== null ? LEVEL_RATIO.clusters : 1;
    this.mix = quantize(zoomMix(this.bandScale));
    this.level = levelAt(this.bandScale);
    this.populate();

    this.scheme = window.matchMedia?.("(prefers-color-scheme: dark)") ?? null;
    const dark = this.scheme?.matches ?? false;
    this.sigma = new Sigma(this.graph, container, {
      defaultNodeType: "pictogram",
      nodeProgramClasses: { pictogram: PictogramProgram },
      edgeProgramClasses: { line: EdgeRectangleProgram, arrow: EdgeArrowProgram },
      renderEdgeLabels: false,
      zIndex: true,
      labelFont: "ui-sans-serif, system-ui, sans-serif",
      labelSize: 13,
      labelWeight: "500",
      labelColor: { color: labelColor(dark) },
      labelRenderedSizeThreshold: 7,
      minCameraRatio: 0.02,
      maxCameraRatio: 3,
      nodeReducer: (node, data) => this.nodeLook(node, data),
      edgeReducer: (edge, data) => this.edgeLook(edge, data),
    });
    // Normalise against the true layout, so positions a reducer animates
    // never rescale the whole map mid-transition.
    const positions = this.model.memories.map((m) => m.position);
    if (positions.length > 0) {
      const xs = positions.map((p) => p.x);
      const ys = positions.map((p) => p.y);
      this.sigma.setCustomBBox({ x: [Math.min(...xs) - 1, Math.max(...xs) + 1], y: [Math.min(...ys) - 1, Math.max(...ys) + 1] });
    }

    this.hulls = this.sigma.createCanvas("hulls", { beforeLayer: "edges" });
    this.lassoCanvas = this.sigma.createCanvas("lasso", { afterLayer: "mouse" });
    this.lassoCanvas.style.pointerEvents = "none";
    this.wireLasso();
    this.sigma.on("afterRender", () => this.drawHulls());
    this.sigma.getCamera().on("updated", (state) => this.onCamera(state.ratio));
    this.sigma.on("clickNode", ({ node }) => this.onClick(node));
    this.sigma.on("enterNode", ({ node }) => this.onHover(node));
    this.sigma.on("leaveNode", () => this.onHover(null));
    // Follow the system theme while the map is open, not only when it is created.
    this.scheme?.addEventListener("change", this.onScheme);
    this.sigma.refresh();
    this.callbacks.onLevelChange(this.level);
  }

  /** Camera ratio × this = the ratio the zoom bands read (see constructor). */
  private readonly bandScale: number;

  private readonly scheme: MediaQueryList | null;
  private readonly onScheme = (event: MediaQueryListEvent) => {
    this.sigma.setSetting("labelColor", { color: labelColor(event.matches) });
  };

  update(view: ViewState): void {
    this.view = view;
    this.sigma.refresh();
  }

  focus(ids: readonly string[]): void {
    const points = ids
      .map((id) => this.sigma.getNodeDisplayData(id))
      .filter((d): d is NodeDisplayData => d !== undefined);
    // Display data are only meaningful once memories are drawn: compute where they will be.
    const target = ids
      .map((id) => this.model.byId.get(id))
      .filter((m) => m !== undefined)
      .map((m) => this.sigma.graphToViewport(m.position))
      .map((p) => this.sigma.viewportToFramedGraph(p));
    const framed = target.length > 0 ? target : points;
    if (framed.length === 0) return;
    const xs = framed.map((p) => p.x);
    const ys = framed.map((p) => p.y);
    const span = Math.max(Math.max(...xs) - Math.min(...xs), Math.max(...ys) - Math.min(...ys));
    // Frame them all: one hit gets a close-up, hits spread over the map get the whole map.
    const ratio = Math.min(1.1, Math.max(0.04, span * 1.4));
    void this.sigma.getCamera().animate(
      { x: (Math.min(...xs) + Math.max(...xs)) / 2, y: (Math.min(...ys) + Math.max(...ys)) / 2, ratio },
      { duration: 600 },
    );
  }

  zoomTo(level: Level): void {
    const camera = this.sigma.getCamera();
    const ratio = LEVEL_RATIO[level] / this.bandScale;
    // Zooming in on the view's centre can land in the empty middle of the
    // category ring. Close in on the category (or cluster) nearest the
    // centre instead, so there is always something to see.
    const groups = level === "clusters" ? this.model.categories : level === "memories" ? this.model.clusters : [];
    const target = ratio < camera.getState().ratio ? this.nearest(groups, camera.getState()) : null;
    void camera.animate(target === null ? { ratio } : { ...target, ratio }, { duration: 500 });
  }

  /** The framed-graph position of the shown group closest to `point`. */
  private nearest(groups: readonly GroupNode[], point: Point): Point | null {
    let best: Point | null = null;
    let distance = Number.POSITIVE_INFINITY;
    for (const m of groups) {
      if (this.view.hiddenCategories.has(m.category)) continue;
      const at = this.sigma.viewportToFramedGraph(this.sigma.graphToViewport(m.position));
      const d = Math.hypot(at.x - point.x, at.y - point.y);
      if (d < distance) {
        distance = d;
        best = at;
      }
    }
    return best;
  }

  zoomIn(): void {
    void this.sigma.getCamera().animatedZoom({ duration: 300, factor: 1.6 });
  }

  zoomOut(): void {
    void this.sigma.getCamera().animatedUnzoom({ duration: 300, factor: 1.6 });
  }

  reset(): void {
    void this.sigma.getCamera().animatedReset({ duration: 500 });
  }

  pan(dx: number, dy: number): void {
    const camera = this.sigma.getCamera();
    const { width, height } = this.sigma.getDimensions();
    const state = camera.getState();
    // Move by a share of what is on screen, so a key press feels the same at every zoom.
    const from = this.sigma.viewportToFramedGraph({ x: width / 2, y: height / 2 });
    const to = this.sigma.viewportToFramedGraph({ x: width / 2 + dx * width, y: height / 2 + dy * height });
    void camera.animate({ x: state.x + (to.x - from.x), y: state.y + (to.y - from.y) }, { duration: 150 });
  }

  setLassoMode(on: boolean): void {
    this.lassoMode = on;
    this.sigma.getContainer().style.cursor = on ? "crosshair" : "";
  }

  destroy(): void {
    this.scheme?.removeEventListener("change", this.onScheme);
    this.sigma.kill();
  }

  // --- lasso

  private wireLasso(): void {
    const captor = this.sigma.getMouseCaptor();
    captor.on("mousedown", (event) => {
      if (!this.lassoMode && !event.original.shiftKey) return;
      this.lasso = [{ x: event.x, y: event.y }];
    });
    captor.on("mousemovebody", (event) => {
      if (this.lasso === null) return;
      // Draw instead of panning.
      event.preventSigmaDefault();
      event.original.preventDefault();
      event.original.stopPropagation();
      const last = this.lasso.at(-1)!;
      if (Math.hypot(event.x - last.x, event.y - last.y) > 3) this.lasso.push({ x: event.x, y: event.y });
      this.drawLasso();
    });
    captor.on("mouseup", () => {
      if (this.lasso === null) return;
      const outline = this.lasso;
      this.lasso = null;
      this.drawLasso();
      if (outline.length >= 3) this.callbacks.onLasso(this.enclosed(outline));
    });
  }

  /** Memories drawn inside `outline`, plus the members of any group disc drawn inside it. */
  private enclosed(outline: Point[]): string[] {
    const visible: { id: string; point: Point }[] = [];
    this.graph.forEachNode((node) => {
      const display = this.sigma.getNodeDisplayData(node);
      if (display === undefined || display.hidden) return;
      visible.push({ id: node, point: this.sigma.framedGraphToViewport({ x: display.x, y: display.y }) });
    });
    const ids = new Set<string>();
    for (const node of lassoed(visible, outline)) {
      const kind = this.graph.getNodeAttribute(node, "kind") as NodeKind;
      if (kind === "memory") ids.add(node);
      else {
        const group = kind === "category" ? this.group(this.model.categories, node) : this.model.clusterById.get(node);
        for (const member of group?.members ?? []) {
          const memory = this.model.byId.get(member);
          if (memory !== undefined && !this.view.hiddenCategories.has(memory.category)) ids.add(member);
        }
      }
    }
    return [...ids];
  }

  private drawLasso(): void {
    const canvas = this.lassoCanvas;
    const { width, height } = this.sigma.getDimensions();
    const ratio = window.devicePixelRatio || 1;
    if (canvas.width !== Math.round(width * ratio) || canvas.height !== Math.round(height * ratio)) {
      canvas.width = Math.round(width * ratio);
      canvas.height = Math.round(height * ratio);
      canvas.style.width = `${width}px`;
      canvas.style.height = `${height}px`;
    }
    const context = canvas.getContext("2d");
    if (context === null) return;
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, width, height);
    if (this.lasso === null || this.lasso.length < 2) return;
    context.beginPath();
    this.lasso.forEach((p, i) => (i === 0 ? context.moveTo(p.x, p.y) : context.lineTo(p.x, p.y)));
    context.closePath();
    context.fillStyle = "rgba(79, 70, 229, 0.08)";
    context.strokeStyle = "rgba(79, 70, 229, 0.9)";
    context.setLineDash([6, 4]);
    context.lineWidth = 1.5;
    context.fill();
    context.stroke();
  }

  // --- graph

  private populate(): void {
    const add = (
      id: string,
      kind: NodeKind,
      category: string,
      label: string,
      position: Point,
      size: number,
      memoryKind?: string,
    ) => {
      // A memory wears its kind; a topic group is neutral (plan D13).
      const style = memoryKind === undefined ? categoryStyle(category) : kindStyle(memoryKind);
      this.graph.addNode(id, {
        kind,
        category,
        x: position.x,
        y: position.y,
        size,
        label,
        color: style.color,
        image: glyphImage(style.glyph, label),
        pictoColor: "#ffffff",
      });
    };
    for (const c of this.model.categories) add(c.id, "category", c.category, c.label, c.position, 10);
    for (const c of this.model.clusters) {
      if (!this.graph.hasNode(c.id)) add(c.id, "cluster", c.category, c.label, c.position, 6);
    }
    for (const m of this.model.memories) add(m.id, "memory", m.category, m.memory.content, m.position, 4, m.memory.kind);

    this.model.edges.forEach((edge, index) => {
      this.graph.addDirectedEdgeWithKey(`e${index}`, edge.source, edge.target, { index, group: "memory" });
    });
    this.model.categoryEdges.forEach((edge, index) => {
      const source = `category:${edge.source}`;
      const target = `category:${edge.target}`;
      if (this.graph.hasNode(source) && this.graph.hasNode(target)) {
        this.graph.addDirectedEdgeWithKey(`c${index}`, source, target, { index, group: "category" });
      }
    });
  }

  private effectiveView(): ViewState {
    if (this.hovered === null) return this.view;
    // Hovering shows the neighbourhood, unless a search is already focusing the map.
    return {
      ...this.view,
      hovered: this.hovered,
      highlighted: this.view.highlighted ?? neighbourhood(this.hovered, this.model, this.view),
    };
  }

  private nodeLook(node: string, data: Record<string, unknown>): Partial<NodeDisplayData> {
    const view = this.effectiveView();
    let look: Look;
    switch (data.kind as NodeKind) {
      case "category":
        look = categoryLook(this.group(this.model.categories, node), this.mix, view);
        break;
      case "cluster":
        look = clusterLook(this.model.clusterById.get(node)!, this.model, this.mix, view);
        break;
      default:
        look = memoryLook(this.model.byId.get(node)!, this.model, this.mix, view);
    }
    return { ...data, ...look, label: look.label };
  }

  private edgeLook(_edge: string, data: Record<string, unknown>) {
    const view = this.effectiveView();
    const look =
      data.group === "category"
        ? categoryEdgeLook(this.model.categoryEdges[data.index as number]!, this.mix, view)
        : edgeLook(this.model.edges[data.index as number]!, this.model, this.mix, view);
    return { ...data, ...look };
  }

  private group(groups: GroupNode[], id: string): GroupNode {
    return groups.find((g) => g.id === id)!;
  }

  // --- interaction

  private onCamera(ratio: number): void {
    // An aggregate map has nothing to dissolve into: it stays on its categories at any zoom.
    if (this.model.aggregate) return;
    const mix = quantize(zoomMix(ratio * this.bandScale));
    if (mix.cluster !== this.mix.cluster || mix.memory !== this.mix.memory) {
      this.mix = mix;
      this.sigma.refresh();
    }
    const level = levelAt(ratio * this.bandScale);
    if (level !== this.level) {
      this.level = level;
      this.callbacks.onLevelChange(level);
    }
  }

  private onClick(node: string): void {
    const kind = this.graph.getNodeAttribute(node, "kind") as NodeKind;
    if (kind === "memory") {
      this.callbacks.onOpenMemory(node);
      return;
    }
    if (kind === "category" && this.model.aggregate) {
      this.callbacks.onOpenCategory(this.graph.getNodeAttribute(node, "category") as string);
      return;
    }
    const display = this.sigma.getNodeDisplayData(node);
    if (display === undefined) return;
    void this.sigma
      .getCamera()
      .animate(
        { x: display.x, y: display.y, ratio: (kind === "category" ? LEVEL_RATIO.clusters : LEVEL_RATIO.memories) / this.bandScale },
        { duration: 600 },
      );
  }

  private onHover(node: string | null): void {
    const memory = node !== null && this.graph.getNodeAttribute(node, "kind") === "memory" ? node : null;
    if (memory === this.hovered) return;
    this.hovered = memory;
    this.sigma.getContainer().style.cursor = node !== null ? "pointer" : this.lassoMode ? "crosshair" : "";
    this.sigma.refresh();
  }

  // --- category hulls, under the edges

  private drawHulls(): void {
    const canvas = this.hulls;
    const { width, height } = this.sigma.getDimensions();
    const ratio = window.devicePixelRatio || 1;
    if (canvas.width !== Math.round(width * ratio) || canvas.height !== Math.round(height * ratio)) {
      canvas.width = Math.round(width * ratio);
      canvas.height = Math.round(height * ratio);
      canvas.style.width = `${width}px`;
      canvas.style.height = `${height}px`;
    }
    const context = canvas.getContext("2d");
    if (context === null) return;
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, width, height);

    const alpha = this.mix.cluster * (1 - 0.7 * this.mix.memory);
    if (alpha < 0.04) return;
    for (const category of this.model.categories) {
      if (this.view.hiddenCategories.has(category.category) || category.members.length === 0) continue;
      const points = category.members
        .map((id) => this.model.byId.get(id)!)
        .map((m) => this.sigma.graphToViewport(memoryPosition(m, this.model, { cluster: 1, memory: 1 })));
      this.drawHull(context, points, categoryStyle(category.category).color, alpha);
    }
  }

  private drawHull(context: CanvasRenderingContext2D, points: Point[], color: string, alpha: number): void {
    const padding = 22;
    const hull = convexHull(points);
    context.fillStyle = withAlpha(color, 0.07 * alpha);
    context.strokeStyle = withAlpha(color, 0.45 * alpha);
    context.lineWidth = 1.5;
    context.beginPath();
    if (hull.length < 3) {
      const centre = hull.reduce((acc, p) => ({ x: acc.x + p.x / hull.length, y: acc.y + p.y / hull.length }), { x: 0, y: 0 });
      const spread = hull.length === 2 ? Math.hypot(hull[0]!.x - hull[1]!.x, hull[0]!.y - hull[1]!.y) / 2 : 0;
      context.arc(centre.x, centre.y, spread + padding, 0, 2 * Math.PI);
    } else {
      // Push each corner away from the centre by the padding, then round the joins.
      const cx = hull.reduce((s, p) => s + p.x, 0) / hull.length;
      const cy = hull.reduce((s, p) => s + p.y, 0) / hull.length;
      const padded = hull.map((p) => {
        const d = Math.hypot(p.x - cx, p.y - cy) || 1;
        return { x: p.x + ((p.x - cx) / d) * padding, y: p.y + ((p.y - cy) / d) * padding };
      });
      context.lineJoin = "round";
      padded.forEach((p, i) => (i === 0 ? context.moveTo(p.x, p.y) : context.lineTo(p.x, p.y)));
      context.closePath();
    }
    context.fill();
    context.stroke();
  }
}
