import type { RenderedImage } from "../core/contracts.js";
import { failure, success, type OperationResult } from "../core/contracts.js";
import { HerdrMathError, serializeError } from "../core/errors.js";
import { encodeValidatedPng, type EncodedPng } from "../graphics/placement.js";
import type { HerdrGraphicsInfo, HerdrGraphicsSetRequest, HerdrPaneLayoutSnapshot } from "../herdr/socket-client.js";
import { stackRenderedImages } from "./stack-images.js";

export interface ViewerPresenterClient {
  paneGraphicsInfo(paneId: string): Promise<OperationResult<HerdrGraphicsInfo>>;
  paneLayout(paneId: string): Promise<OperationResult<HerdrPaneLayoutSnapshot>>;
  paneGraphicsSet(request: HerdrGraphicsSetRequest): Promise<OperationResult<void>>;
}

export interface ViewerPresentationRequest {
  viewerPaneId: string;
  workspaceId: string;
  image: RenderedImage;
}

interface ViewerGeometry {
  info: HerdrGraphicsInfo;
  paneRect: { width: number; height: number };
}

export class ViewerPresenter {
  #accumulatedImage: RenderedImage | undefined;
  #scrollOffsetFromBottomRows = 0;
  #lastViewerPaneId: string | undefined;

  constructor(private readonly client: ViewerPresenterClient) {}

  async present(request: ViewerPresentationRequest): Promise<OperationResult<void>> {
    try {
      if (this.#lastViewerPaneId !== undefined && this.#lastViewerPaneId !== request.viewerPaneId) {
        this.#accumulatedImage = undefined;
        this.#scrollOffsetFromBottomRows = 0;
      }
      this.#lastViewerPaneId = request.viewerPaneId;

      const stacked = await stackRenderedImages(this.#accumulatedImage, request.image);
      const geometry = await this.#geometry(request);
      if (!geometry.ok) return failure(geometry.error);
      this.#accumulatedImage = stacked;
      this.#scrollOffsetFromBottomRows = 0;
      return this.#render(request.viewerPaneId, stacked, geometry.value, 0);
    } catch (error) {
      return failure(serializeError(error));
    }
  }

  async scrollBy(viewerPaneId: string, workspaceId: string, deltaRows: number): Promise<OperationResult<void>> {
    try {
      if (this.#accumulatedImage === undefined || this.#lastViewerPaneId !== viewerPaneId) {
        return success(undefined);
      }
      if (!Number.isSafeInteger(deltaRows) || deltaRows === 0) return success(undefined);
      const geometry = await this.#geometry({ viewerPaneId, workspaceId, image: this.#accumulatedImage });
      if (!geometry.ok) return failure(geometry.error);
      const overflowRows = this.#overflowRows(this.#accumulatedImage, geometry.value);
      if (overflowRows <= 0) return success(undefined);
      const next = clamp(this.#scrollOffsetFromBottomRows + deltaRows, 0, overflowRows);
      if (next === this.#scrollOffsetFromBottomRows) return success(undefined);
      this.#scrollOffsetFromBottomRows = next;
      return this.#render(viewerPaneId, this.#accumulatedImage, geometry.value, next);
    } catch (error) {
      return failure(serializeError(error));
    }
  }

  async reflow(viewerPaneId: string, workspaceId: string, image: RenderedImage): Promise<OperationResult<void>> {
    try {
      if (this.#lastViewerPaneId !== viewerPaneId) return success(undefined);
      const geometry = await this.#geometry({ viewerPaneId, workspaceId, image });
      if (!geometry.ok) return failure(geometry.error);
      this.#accumulatedImage = image;
      this.#scrollOffsetFromBottomRows = 0;
      return this.#render(viewerPaneId, image, geometry.value, 0);
    } catch (error) {
      return failure(serializeError(error));
    }
  }

  async #geometry(request: ViewerPresentationRequest): Promise<OperationResult<ViewerGeometry>> {
    const info = await this.client.paneGraphicsInfo(request.viewerPaneId);
    if (!info.ok) return failure(info.error);
    const layout = await this.client.paneLayout(request.viewerPaneId);
    if (!layout.ok) return failure(layout.error);
    if (layout.value.workspaceId !== request.workspaceId) return ownershipFailure();
    const panes = layout.value.panes.filter(({ paneId }) => paneId === request.viewerPaneId);
    const pane = panes[0];
    if (panes.length !== 1 || pane === undefined) return ownershipFailure();
    return success({ info: info.value, paneRect: { width: pane.rect.width, height: pane.rect.height } });
  }

  #overflowRows(image: RenderedImage, geometry: ViewerGeometry): number {
    const naturalRows = Math.ceil(image.height / geometry.info.cellHeightPx);
    return Math.max(0, naturalRows - geometry.paneRect.height);
  }

  async #render(
    viewerPaneId: string,
    image: RenderedImage,
    geometry: ViewerGeometry,
    offsetFromBottomRows: number
  ): Promise<OperationResult<void>> {
    const encoded = encodeValidatedPng(image);
    if (!encoded.ok) return failure(encoded.error);
    const placement = computeScrollablePlacement(encoded.value, geometry.info, geometry.paneRect, offsetFromBottomRows);
    if (!placement.ok) return failure(placement.error);
    return this.client.paneGraphicsSet({
      paneId: viewerPaneId,
      imageWidth: encoded.value.width,
      imageHeight: encoded.value.height,
      dataBase64: encoded.value.dataBase64,
      placement: placement.value
    });
  }
}

function computeScrollablePlacement(
  image: Pick<EncodedPng, "width" | "height">,
  info: HerdrGraphicsInfo,
  paneRect: { width: number; height: number },
  offsetFromBottomRows: number
): OperationResult<HerdrGraphicsSetRequest["placement"]> {
  try {
    if (
      !Number.isSafeInteger(info.cellWidthPx) ||
      !Number.isSafeInteger(info.cellHeightPx) ||
      info.cellWidthPx <= 0 ||
      info.cellHeightPx <= 0 ||
      !Number.isSafeInteger(paneRect.width) ||
      !Number.isSafeInteger(paneRect.height) ||
      paneRect.width <= 0 ||
      paneRect.height <= 0
    ) {
      throw new HerdrMathError("cell_size_unavailable");
    }
    const naturalCols = Math.ceil(image.width / info.cellWidthPx);
    const naturalRows = Math.ceil(image.height / info.cellHeightPx);
    const overflowRows = Math.max(0, naturalRows - paneRect.height);
    const clampedOffset = clamp(offsetFromBottomRows, 0, overflowRows);
    return success(
      Object.freeze({
        viewportCol: 0,
        viewportRow: -(overflowRows - clampedOffset),
        gridCols: Math.max(1, Math.min(paneRect.width, naturalCols)),
        gridRows: Math.max(1, naturalRows)
      })
    );
  } catch (error) {
    return failure(serializeError(error));
  }
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(maximum, Math.max(minimum, value));
}

function ownershipFailure<T>(): OperationResult<T> {
  return failure(serializeError(new HerdrMathError("viewer_ownership_failed")));
}
