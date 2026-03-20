import { AxiosError, AxiosRequestConfig, isAxiosError } from "axios";

function safeToJSON(value: any) {
    if (typeof Buffer !== "undefined" && Buffer.isBuffer?.(value)) {
        return `<Buffer ${value.length} bytes>`;
    }

    if (value instanceof ArrayBuffer) {
        return `<ArrayBuffer ${value.byteLength} bytes>`;
    }

    if (ArrayBuffer.isView(value)) {
        return `<TypedArray ${value.byteLength} bytes>`;
    }

    if (typeof FormData !== "undefined" && value instanceof FormData) {
        return "<FormData>";
    }

    return value;
}

function stringify(value: any) {
    try {
        return typeof value === "string" ? value : JSON.stringify(value, (k, v) => safeToJSON(v), 2);
    } catch {
        return String(value);
    }
}

function fullUrlFromConfig(cfg?: Pick<AxiosRequestConfig, "baseURL" | "url">) {
    const u = cfg?.url ?? "";
    const base = cfg?.baseURL;
    try {
        return base ? new URL(u || "", base).toString() : u;
    } catch {
        return base ? `${base.replace(/\/+$/, "")}/${(u || "").replace(/^\/+/, "")}` : u;
    }
}

export function logAxiosError(err: unknown, label = "HTTP error") {
    if (!isAxiosError(err)) {
        console.error(label, { error: err });
        return;
    }

    const cfg = (err as AxiosError).config as AxiosRequestConfig | undefined;

    const method = (cfg?.method ?? "GET").toUpperCase();
    const url = fullUrlFromConfig(cfg);

    const requestBody =
        cfg?.data !== undefined
            ? cfg.data
            : cfg?.params
                ? { params: cfg.params }
                : undefined;

    if (err.response) {
        console.error(label, {
            method,
            url,
            requestBody: requestBody !== undefined ? stringify(requestBody) : undefined,
            status: err.response.status,
            responseBody: stringify(err.response.data),
        });
    } else if (err.request) {
        console.error(label, {
            method,
            url,
            requestBody: requestBody !== undefined ? stringify(requestBody) : undefined,
            status: null,
            responseBody: "<no response>",
        });
    } else {
        console.error(label, {
            method,
            url,
            requestBody: requestBody !== undefined ? stringify(requestBody) : undefined,
            status: null,
            responseBody: err.message,
        });
    }
}
