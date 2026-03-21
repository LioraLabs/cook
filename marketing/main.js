import "./style.css";
import { initFireShader } from "./fire-shader.js";

const canvas = document.getElementById("fire-canvas");
if (canvas) {
  initFireShader(canvas);
}

// Waitlist form handling
document.querySelectorAll(".waitlist").forEach((form) => {
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const input = form.querySelector("input[type=email]");
    const button = form.querySelector("button");
    const email = input.value;

    button.textContent = "Sending...";
    button.disabled = true;

    try {
      const res = await fetch(form.action, {
        method: "POST",
        headers: { "Content-Type": "application/json", Accept: "application/json" },
        body: JSON.stringify({ email }),
      });

      if (res.ok) {
        button.textContent = "You're in! 🍳";
        input.value = "";
        input.disabled = true;
      } else {
        throw new Error("Form submission failed");
      }
    } catch {
      button.textContent = "Try again";
      button.disabled = false;
    }
  });
});

// Smooth scroll for anchor links
document.querySelectorAll('a[href^="#"]').forEach((link) => {
  link.addEventListener("click", (e) => {
    const target = document.querySelector(link.getAttribute("href"));
    if (target) {
      e.preventDefault();
      target.scrollIntoView({ behavior: "smooth" });
    }
  });
});
