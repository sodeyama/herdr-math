import type { RenderedImage } from "../core/contracts.js";
import { failure, success, type OperationResult } from "../core/contracts.js";
import { HerdrMathError, serializeError } from "../core/errors.js";
import { POLICY_LIMITS } from "../core/limits.js";
import { computeGraphicsPlacement, encodeValidatedPng, type EncodedPng } from "../graphics/placement.js";
import type { HerdrGraphicsInfo, HerdrGraphicsSetRequest, HerdrPaneLayoutSnapshot } from "../herdr/socket-client.js";
import sharp from "sharp";
import { planScrollFrames } from "./scroll-frames.js";

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

interface PreparedFrame {
  encoded: EncodedPng;
  placement: HerdrGraphicsSetRequest["placement"];
}

export class ViewerPresenter {
  #previousFinalFrame: PreparedFrame | undefined;

  constructor(
    private readonly client: ViewerPresenterClient,
    private readonly sleep: (milliseconds: number) => Promise<void> = defaultSleep
  ) {}

  async present(request: ViewerPresentationRequest): Promise<OperationResult<void>> {
    try {
      const prepared = await this.#prepare(request);
      if (!prepared.ok) return failure(prepared.error);
      for (let index = 0; index < prepared.value.length; index += 1) {
        const frame = prepared.value[index];
        if (frame === undefined) throw new HerdrMathError("renderer_failed", {}, true);
        const updated = await this.client.paneGraphicsSet(toGraphicsRequest(request.viewerPaneId, frame));
        if (!updated.ok) {
          await this.#restorePrevious(request.viewerPaneId);
          return failure(updated.error);
        }
        if (index + 1 < prepared.value.length) await this.sleep(POLICY_LIMITS.scrollFrameIntervalMs);
      }
      this.#previousFinalFrame = prepared.value.at(-1);
      return success(undefined);
    } catch (error) {
      return failure(serializeError(error));
    }
  }

  async #prepare(request: ViewerPresentationRequest): Promise<OperationResult<readonly PreparedFrame[]>> {
    const encoded = encodeValidatedPng(request.image);
    if (!encoded.ok) return failure(encoded.error);
    const info = await this.client.paneGraphicsInfo(request.viewerPaneId);
    if (!info.ok) return failure(info.error);
    const layout = await this.client.paneLayout(request.viewerPaneId);
    if (!layout.ok) return failure(layout.error);
    if (layout.value.workspaceId !== request.workspaceId) return ownershipFailure();
    const panes = layout.value.panes.filter(({ paneId }) => paneId === request.viewerPaneId);
    const pane = panes[0];
    if (panes.length !== 1 || pane === undefined) return ownershipFailure();

    const plan = planScrollFrames(
      { width: encoded.value.width, height: encoded.value.height },
      { widthPx: pane.rect.width * info.value.cellWidthPx, heightPx: pane.rect.height * info.value.cellHeightPx }
    );
    if (!plan.ok) return failure(plan.error);

    const frames: PreparedFrame[] = [];
    let aggregateBytes = 0;
    for (const offset of plan.value.offsetsPx) {
      const frameImage =
        plan.value.offsetsPx.length === 1
          ? request.image
          : await cropFrame(request.image, offset, plan.value.frameHeightPx);
      aggregateBytes += frameImage.bytes;
      if (aggregateBytes > POLICY_LIMITS.scrollFrameAggregateBytes) {
        return failure(
          serializeError(
            new HerdrMathError("image_too_large", {
              limit_kind: "scroll_frame_aggregate_bytes",
              limit: POLICY_LIMITS.scrollFrameAggregateBytes,
              actual: aggregateBytes
            })
          )
        );
      }
      const frameEncoded = encodeValidatedPng(frameImage);
      if (!frameEncoded.ok) return failure(frameEncoded.error);
      const placement = computeGraphicsPlacement(frameEncoded.value, info.value, pane.rect);
      if (!placement.ok) return failure(placement.error);
      frames.push({ encoded: frameEncoded.value, placement: placement.value });
    }
    return success(Object.freeze(frames));
  }

  async #restorePrevious(viewerPaneId: string): Promise<void> {
    if (this.#previousFinalFrame === undefined) return;
    await this.client.paneGraphicsSet(toGraphicsRequest(viewerPaneId, this.#previousFinalFrame));
  }
}

async function cropFrame(image: RenderedImage, top: number, height: number): Promise<RenderedImage> {
  try {
    const output = await sharp(image.buffer, { limitInputPixels: POLICY_LIMITS.imagePixels })
      .extract({ left: 0, top, width: image.width, height })
      .png({ adaptiveFiltering: true, compressionLevel: 9 })
      .toBuffer({ resolveWithObject: true });
    return {
      buffer: output.data,
      width: output.info.width,
      height: output.info.height,
      bytes: output.data.byteLength,
      renderer: image.renderer
    };
  } catch {
    throw new HerdrMathError("renderer_failed", {}, true);
  }
}

function toGraphicsRequest(paneId: string, frame: PreparedFrame): HerdrGraphicsSetRequest {
  return {
    paneId,
    imageWidth: frame.encoded.width,
    imageHeight: frame.encoded.height,
    dataBase64: frame.encoded.dataBase64,
    placement: frame.placement
  };
}

function ownershipFailure<T>(): OperationResult<T> {
  return failure(serializeError(new HerdrMathError("viewer_ownership_failed")));
}

function defaultSleep(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
