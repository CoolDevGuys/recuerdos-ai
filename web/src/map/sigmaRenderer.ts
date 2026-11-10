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
import { convexHull, type Point } from "../domain/map/geometry";
import type { GroupNode, MapModel } from "../domain/map/layout";
import { categoryStyle, glyphImage, withAlpha } from "../domain/map/palette";
import { LEVEL_RATIO, levelAt, quantize, zoomMix, type Level, type ZoomMix } from "../domain/map/zoom";
import type { MapRenderer, RendererCallbacks, RendererFactory } from "./renderer";

type NodeKind = "memory" | "category" | "cluster";

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

  constructor(
    container: HTMLElement,
    private readonly model: MapModel,
    view: ViewState,
    private readonly callbacks: RendererCallbacks,
  ) {
    this.view = view;
    this.mix = quantize(zoomMix(1));
    this.level = "categories";
    this.populate();

    const dark = window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false;
    this.sigma = new Sigma(this.graph, container, {
      defaultNodeType: "pictogram",
      nodeProgramClasses: { pictogram: PictogramProgram },
      edgeProgramClasses: { line: EdgeRectangleProgram, arrow: EdgeArrowProgram },
      renderEdgeLabels: false,
      zIndex: true,
      labelFont: "ui-sans-serif, system-ui, sans-serif",
      labelSize: 13,
      labelWeight: "500",
      labelColor: { color: dark ? "#e2e8f0" : "#0f172a" },
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
    this.sigma.on("afterRender", () => this.drawHulls());
    this.sigma.getCamera().on("updated", (state) => this.onCamera(state.ratio));
    this.sigma.on("clickNode", ({ node }) => this.onClick(node));
    this.sigma.on("enterNode", ({ node }) => this.onHover(node));
    this.sigma.on("leaveNode", () => this.onHover(null));
    this.sigma.refresh();
  }

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
    void this.sigma.getCamera().animate({ ratio: LEVEL_RATIO[level] }, { duration: 500 });
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

  destroy(): void {
    this.sigma.kill();
  }

  // --- graph

  private populate(): void {
    const add = (id: string, kind: NodeKind, category: string, label: string, position: Point, size: number) => {
      const style = categoryStyle(category);
      this.graph.addNode(id, {
        kind,
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
    for (const m of this.model.memories) add(m.id, "memory", m.category, m.memory.content, m.position, 4);

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
    const mix = quantize(zoomMix(ratio));
    if (mix.cluster !== this.mix.cluster || mix.memory !== this.mix.memory) {
      this.mix = mix;
      this.sigma.refresh();
    }
    const level = levelAt(ratio);
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
    const display = this.sigma.getNodeDisplayData(node);
    if (display === undefined) return;
    void this.sigma
      .getCamera()
      .animate(
        { x: display.x, y: display.y, ratio: kind === "category" ? LEVEL_RATIO.clusters : LEVEL_RATIO.memories },
        { duration: 600 },
      );
  }

  private onHover(node: string | null): void {
    const memory = node !== null && this.graph.getNodeAttribute(node, "kind") === "memory" ? node : null;
    if (memory === this.hovered) return;
    this.hovered = memory;
    this.sigma.getContainer().style.cursor = node !== null ? "pointer" : "";
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
      if (this.view.hiddenCategories.has(category.category)) continue;
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
