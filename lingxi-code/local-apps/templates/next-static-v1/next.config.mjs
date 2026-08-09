const outputMode = process.env.LINGXI_APP_OUTPUT;
const contentSecurityPolicy = [
  "default-src 'self'",
  "script-src 'self' 'unsafe-inline'",
  "style-src 'self' 'unsafe-inline'",
  "img-src 'self' data: blob:",
  "font-src 'self' data:",
  "connect-src 'self'",
  "media-src 'self' data: blob:",
  "worker-src 'none'",
  "object-src 'none'",
  "base-uri 'none'",
  "frame-ancestors 'none'",
  "form-action 'self'",
].join("; ");

if (outputMode !== "export" && outputMode !== "server") {
  throw new Error("LINGXI_APP_OUTPUT must be either export or server");
}

/** @type {import('next').NextConfig} */
const nextConfig = {
  ...(outputMode === "export" ? { output: "export" } : {}),
  images: { unoptimized: true },
  poweredByHeader: false,
  reactStrictMode: true,
  trailingSlash: true,
  ...(outputMode === "server"
    ? {
        async headers() {
          return [
            {
              source: "/:path*",
              headers: [
                {
                  key: "Content-Security-Policy",
                  value: contentSecurityPolicy,
                },
                { key: "Referrer-Policy", value: "no-referrer" },
                { key: "X-Content-Type-Options", value: "nosniff" },
                { key: "X-Frame-Options", value: "DENY" },
              ],
            },
          ];
        },
      }
    : {}),
};

export default nextConfig;
