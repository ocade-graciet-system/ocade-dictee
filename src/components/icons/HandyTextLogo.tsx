import React from "react";

/**
 * OCADE DICTÉE wordmark. Rendered as text inside an SVG so it scales with the
 * same `width`/`height` props the previous "handy" logo used. "OCADE" sits on
 * top, "DICTÉE" underneath as a tracked-out subtitle. Fill follows the themed
 * `--color-logo-primary` token via the shared `.logo-primary` class.
 */
// Wordmark text kept as constants so the i18next lint rule (which targets
// literal JSX text) does not treat the brand name as translatable copy.
const WORDMARK_PRIMARY = "OCADE";
const WORDMARK_SECONDARY = "DICTÉE";

const HandyTextLogo = ({
  width,
  height,
  className,
}: {
  width?: number;
  height?: number;
  className?: string;
}) => {
  return (
    <svg
      width={width}
      height={height}
      className={className}
      viewBox="0 0 340 150"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
    >
      <text
        x="2"
        y="66"
        className="logo-primary"
        fontFamily="'Poppins', system-ui, -apple-system, 'Segoe UI', sans-serif"
        fontWeight={800}
        fontSize={72}
        letterSpacing={2}
      >
        {WORDMARK_PRIMARY}
      </text>
      <text
        x="4"
        y="128"
        className="logo-primary"
        fontFamily="'Poppins', system-ui, -apple-system, 'Segoe UI', sans-serif"
        fontWeight={600}
        fontSize={44}
        letterSpacing={14}
      >
        {WORDMARK_SECONDARY}
      </text>
    </svg>
  );
};

export default HandyTextLogo;
