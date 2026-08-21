// High-performance Particle, Combat, and Environmental FX Engine for Crucible.
// Handles vehicle track trails, bullet tracers, artillery arcs, explosions,
// scorch marks, mining lasers, refinery docking transfers, and death animations.

export interface TrackSegment {
  lx1: number;
  ly1: number;
  lx2: number;
  ly2: number;
  rx1: number;
  ry1: number;
  rx2: number;
  ry2: number;
  life: number;
  maxLife: number;
  isCrawler: boolean;
}

export interface Projectile {
  id: number;
  kind: "bullet" | "shell" | "artillery" | "laser";
  fromX: number;
  fromY: number;
  toX: number;
  toY: number;
  progress: number; // 0 to 1
  speed: number;
  arcHeight: number;
  color: string;
}

export interface Particle {
  x: number;
  y: number;
  vx: number;
  vy: number;
  life: number;
  maxLife: number;
  size: number;
  color: string;
  alpha: number;
}

export interface Explosion {
  x: number;
  y: number;
  radius: number;
  maxRadius: number;
  life: number;
  maxLife: number;
  kind: "small" | "medium" | "heavy" | "building";
}

export interface ScorchMark {
  x: number;
  y: number;
  radius: number;
  life: number;
  maxLife: number;
}

export interface DeathEffect {
  x: number;
  y: number;
  angle: number;
  kind: string;
  owner: number;
  life: number;
  maxLife: number;
}

export class FXEngine {
  tracks: TrackSegment[] = [];
  projectiles: Projectile[] = [];
  particles: Particle[] = [];
  explosions: Explosion[] = [];
  scorchMarks: ScorchMark[] = [];
  deaths: DeathEffect[] = [];

  private nextProjId = 1;
  private lastVehiclePos = new Map<number, { x: number; y: number; angle: number }>();
  private lastUnitFired = new Map<number, number>();

  /** Record that an entity just fired a weapon at a tick */
  recordUnitFiring(id: number, tick: number): void {
    this.lastUnitFired.set(id, tick);
  }

  /** Return ticks elapsed since this unit fired (or -1 if idle) */
  getFiringAge(id: number, currentTick: number): number {
    const t = this.lastUnitFired.get(id);
    if (t == null) return -1;
    const diff = currentTick - t;
    return diff >= 0 && diff <= 10 ? diff : -1;
  }

  /** Step all FX forward in time (dt in seconds) */
  update(dt: number): void {
    // 1. Update tracks
    for (let i = this.tracks.length - 1; i >= 0; i--) {
      this.tracks[i].life -= dt;
      if (this.tracks[i].life <= 0) this.tracks.splice(i, 1);
    }

    // 2. Update scorch marks
    for (let i = this.scorchMarks.length - 1; i >= 0; i--) {
      this.scorchMarks[i].life -= dt;
      if (this.scorchMarks[i].life <= 0) this.scorchMarks.splice(i, 1);
    }

    // 3. Update projectiles
    for (let i = this.projectiles.length - 1; i >= 0; i--) {
      const p = this.projectiles[i];
      p.progress += p.speed * dt;
      if (p.progress >= 1) {
        // Projectile arrived -> trigger impact
        if (p.kind === "artillery") {
          this.spawnExplosion(p.toX, p.toY, "heavy");
        } else if (p.kind === "shell") {
          this.spawnExplosion(p.toX, p.toY, "medium");
        } else {
          this.spawnImpactSparks(p.toX, p.toY, p.color);
        }
        this.projectiles.splice(i, 1);
      }
    }

    // 4. Update explosions
    for (let i = this.explosions.length - 1; i >= 0; i--) {
      this.explosions[i].life -= dt;
      if (this.explosions[i].life <= 0) this.explosions.splice(i, 1);
    }

    // 5. Update death animations
    for (let i = this.deaths.length - 1; i >= 0; i--) {
      this.deaths[i].life -= dt;
      if (this.deaths[i].life <= 0) this.deaths.splice(i, 1);
    }

    // 6. Update particles
    for (let i = this.particles.length - 1; i >= 0; i--) {
      const pt = this.particles[i];
      pt.x += pt.vx * dt;
      pt.y += pt.vy * dt;
      pt.life -= dt;
      if (pt.life <= 0) this.particles.splice(i, 1);
    }
  }

  /** Record track trail segment when vehicles move */
  recordVehicleMovement(id: number, kind: string, x: number, y: number, angle: number): void {
    if (kind === "Infantry") return; // Infantry don't leave wide tank treads

    const last = this.lastVehiclePos.get(id);
    if (!last) {
      this.lastVehiclePos.set(id, { x, y, angle });
      return;
    }

    const dist = Math.hypot(x - last.x, y - last.y);
    if (dist >= 0.12) {
      const isCrawler = kind === "Artillery";
      const halfSpacing = isCrawler ? 0.26 : 0.2;

      // Compute exact left and right tread positions at previous and current points
      const pSin = Math.sin(last.angle);
      const pCos = Math.cos(last.angle);
      const cSin = Math.sin(angle);
      const cCos = Math.cos(angle);

      const lx1 = last.x - pSin * halfSpacing;
      const ly1 = last.y + pCos * halfSpacing;
      const rx1 = last.x + pSin * halfSpacing;
      const ry1 = last.y - pCos * halfSpacing;

      const lx2 = x - cSin * halfSpacing;
      const ly2 = y + cCos * halfSpacing;
      const rx2 = x + cSin * halfSpacing;
      const ry2 = y - cCos * halfSpacing;

      this.tracks.push({
        lx1,
        ly1,
        lx2,
        ly2,
        rx1,
        ry1,
        rx2,
        ry2,
        life: 20,
        maxLife: 20,
        isCrawler,
      });
      if (this.tracks.length > 500) this.tracks.shift();
      this.lastVehiclePos.set(id, { x, y, angle });
    }
  }

  /** Spawn projectile attack streak */
  spawnAttack(
    fromX: number,
    fromY: number,
    toX: number,
    toY: number,
    kind: "bullet" | "shell" | "artillery" | "laser",
    color: string = "#fef08a",
  ): void {
    const speeds = {
      bullet: 18,
      shell: 14,
      artillery: 8,
      laser: 30,
    };
    const dist = Math.hypot(toX - fromX, toY - fromY);
    const speed = speeds[kind] / Math.max(1, dist);

    this.projectiles.push({
      id: this.nextProjId++,
      kind,
      fromX,
      fromY,
      toX,
      toY,
      progress: 0,
      speed,
      arcHeight: kind === "artillery" ? Math.min(3.5, dist * 0.4) : 0,
      color,
    });

    // Muzzle flash at origin
    this.spawnMuzzleFlash(fromX, fromY, color);
  }

  /** Spawn muzzle flash */
  spawnMuzzleFlash(x: number, y: number, color: string): void {
    for (let i = 0; i < 4; i++) {
      const a = Math.random() * Math.PI * 2;
      const spd = 1 + Math.random() * 2;
      this.particles.push({
        x,
        y,
        vx: Math.cos(a) * spd,
        vy: Math.sin(a) * spd,
        life: 0.12,
        maxLife: 0.12,
        size: 3,
        color,
        alpha: 1,
      });
    }
  }

  /** Spawn small impact spark burst */
  spawnImpactSparks(x: number, y: number, color: string): void {
    for (let i = 0; i < 6; i++) {
      const a = Math.random() * Math.PI * 2;
      const spd = 2 + Math.random() * 4;
      this.particles.push({
        x,
        y,
        vx: Math.cos(a) * spd,
        vy: Math.sin(a) * spd,
        life: 0.2,
        maxLife: 0.2,
        size: 2,
        color,
        alpha: 1,
      });
    }
  }

  /** Spawn explosion and ground scorch mark */
  spawnExplosion(x: number, y: number, kind: "small" | "medium" | "heavy" | "building"): void {
    const radii = {
      small: 0.35,
      medium: 0.65,
      heavy: 0.95,
      building: 1.3,
    };
    const maxR = radii[kind];
    const duration = kind === "building" ? 0.65 : 0.45;

    this.explosions.push({
      x,
      y,
      radius: 0.1,
      maxRadius: maxR,
      life: duration,
      maxLife: duration,
      kind,
    });

    // Gritty scorch crater
    this.scorchMarks.push({
      x,
      y,
      radius: maxR * 0.8,
      life: 30,
      maxLife: 30,
    });
    if (this.scorchMarks.length > 150) this.scorchMarks.shift();

    // Searing sparks and black smoke particles
    const particleCount = kind === "building" ? 20 : kind === "heavy" ? 12 : 6;
    for (let i = 0; i < particleCount; i++) {
      const a = Math.random() * Math.PI * 2;
      const spd = 1.5 + Math.random() * 4;
      const isSmoke = Math.random() < 0.6;
      this.particles.push({
        x,
        y,
        vx: Math.cos(a) * spd,
        vy: Math.sin(a) * spd - (isSmoke ? 0.8 : 0.2),
        life: isSmoke ? 0.5 + Math.random() * 0.5 : 0.2 + Math.random() * 0.25,
        maxLife: isSmoke ? 1.0 : 0.45,
        size: isSmoke ? 3 + Math.random() * 3 : 1.5,
        color: isSmoke ? (Math.random() < 0.5 ? "#18181b" : "#27272a") : (Math.random() < 0.5 ? "#facc15" : "#ea580c"),
        alpha: isSmoke ? 0.85 : 1,
      });
    }
  }

  /** Spawn unit death effect */
  spawnDeath(x: number, y: number, angle: number, kind: string, owner: number): void {
    this.deaths.push({
      x,
      y,
      angle,
      kind,
      owner,
      life: 18,
      maxLife: 18,
    });
    if (this.deaths.length > 80) this.deaths.shift();

    if (kind === "Infantry") {
      this.spawnImpactSparks(x, y, "#ef4444");
    } else {
      this.spawnExplosion(x, y, kind === "Hq" ? "building" : "heavy");
    }
  }

  // -------------------------------------------------------------------------
  // Rendering
  // -------------------------------------------------------------------------

  /** Render track marks and scorch marks (drawn ON TOP of terrain, UNDER entities) */
  drawGroundLayer(
    ctx: CanvasRenderingContext2D,
    cam: { screenX: (wx: number) => number; screenY: (wy: number) => number; zoom: number },
    w: number,
    h: number,
  ): void {
    const z = cam.zoom;

    // 1. Scorch Marks (Realistic charred craters)
    for (const sm of this.scorchMarks) {
      const sx = cam.screenX(sm.x);
      const sy = cam.screenY(sm.y);
      const sr = sm.radius * z;
      if (sx < -sr || sy < -sr || sx > w + sr || sy > h + sr) continue;

      const alpha = Math.min(0.7, (sm.life / sm.maxLife) * 0.75);
      ctx.fillStyle = `rgba(10, 10, 12, ${alpha})`;
      ctx.beginPath();
      ctx.arc(sx, sy, sr, 0, Math.PI * 2);
      ctx.fill();

      ctx.fillStyle = `rgba(4, 4, 6, ${alpha * 0.9})`;
      ctx.beginPath();
      ctx.arc(sx, sy, sr * 0.5, 0, Math.PI * 2);
      ctx.fill();
    }

    // 2. Vehicle Track Marks (Continuous, subtle, realistic caterpillar tread ruts)
    ctx.save();
    ctx.lineCap = "round";
    for (const tr of this.tracks) {
      const slx1 = cam.screenX(tr.lx1);
      const sly1 = cam.screenY(tr.ly1);
      const slx2 = cam.screenX(tr.lx2);
      const sly2 = cam.screenY(tr.ly2);
      const srx1 = cam.screenX(tr.rx1);
      const sry1 = cam.screenY(tr.ry1);
      const srx2 = cam.screenX(tr.rx2);
      const sry2 = cam.screenY(tr.ry2);

      const minX = Math.min(slx1, slx2, srx1, srx2);
      const maxX = Math.max(slx1, slx2, srx1, srx2);
      const minY = Math.min(sly1, sly2, sry1, sry2);
      const maxY = Math.max(sly1, sly2, sry1, sry2);
      if (maxX < 0 || minX > w || maxY < 0 || minY > h) continue;

      const alpha = Math.min(0.35, (tr.life / tr.maxLife) * 0.35);
      ctx.strokeStyle = `rgba(10, 16, 12, ${alpha})`;
      ctx.lineWidth = Math.max(1.5, Math.min(3.5, z * (tr.isCrawler ? 0.08 : 0.06)));

      // Left tread path
      ctx.beginPath();
      ctx.moveTo(slx1, sly1);
      ctx.lineTo(slx2, sly2);
      ctx.stroke();

      // Right tread path
      ctx.beginPath();
      ctx.moveTo(srx1, sry1);
      ctx.lineTo(srx2, sry2);
      ctx.stroke();
    }
    ctx.restore();
  }

  /** Render air layer FX: Lasers, Projectiles, Explosions, Smoke, Particles */
  drawAirLayer(
    ctx: CanvasRenderingContext2D,
    cam: { screenX: (wx: number) => number; screenY: (wy: number) => number; zoom: number },
    _w: number,
    _h: number,
  ): void {
    const z = cam.zoom;

    // 1. Projectiles
    for (const p of this.projectiles) {
      const curX = p.fromX + (p.toX - p.fromX) * p.progress;
      const curY = p.fromY + (p.toY - p.fromY) * p.progress;

      // Parabolic arc offset for artillery
      const arcOffset = p.arcHeight * 4 * p.progress * (1 - p.progress);
      const sx = cam.screenX(curX);
      const sy = cam.screenY(curY - arcOffset);

      if (p.kind === "laser") {
        // High-energy laser beam
        ctx.strokeStyle = p.color;
        ctx.lineWidth = 2;
        ctx.beginPath();
        ctx.moveTo(cam.screenX(p.fromX), cam.screenY(p.fromY));
        ctx.lineTo(cam.screenX(p.toX), cam.screenY(p.toY));
        ctx.stroke();
      } else if (p.kind === "artillery") {
        // Artillery shell with shadow
        const shadowSx = cam.screenX(curX);
        const shadowSy = cam.screenY(curY);
        ctx.fillStyle = "rgba(0, 0, 0, 0.4)";
        ctx.beginPath();
        ctx.arc(shadowSx, shadowSy, 2.5, 0, Math.PI * 2);
        ctx.fill();

        // High shell
        ctx.fillStyle = "#facc15";
        ctx.fillRect(sx - 2, sy - 2, 4, 4);
      } else {
        // High-speed kinetic tracer shell streak
        const tailProgress = Math.max(0, p.progress - 0.12);
        const tailX = p.fromX + (p.toX - p.fromX) * tailProgress;
        const tailY = p.fromY + (p.toY - p.fromY) * tailProgress;

        ctx.strokeStyle = p.color;
        ctx.lineWidth = p.kind === "shell" ? 2 : 1.2;
        ctx.beginPath();
        ctx.moveTo(cam.screenX(tailX), cam.screenY(tailY));
        ctx.lineTo(sx, sy);
        ctx.stroke();
      }
    }

    // 2. Realistic Military Explosions (Tight, intense fireball & black smoke clouds)
    for (const exp of this.explosions) {
      const sx = cam.screenX(exp.x);
      const sy = cam.screenY(exp.y);
      const frac = 1 - exp.life / exp.maxLife; // 0 to 1
      const maxPx = exp.maxRadius * z;

      ctx.save();
      ctx.translate(sx, sy);

      if (frac < 0.35) {
        // Initial brilliant white-hot blast flash
        const flashR = maxPx * (0.4 + frac * 1.5);
        ctx.fillStyle = "#ffffff";
        ctx.beginPath();
        ctx.arc(0, 0, flashR * 0.6, 0, Math.PI * 2);
        ctx.fill();

        // Jagged fiery burst spikes
        ctx.fillStyle = "#fde047";
        ctx.beginPath();
        for (let i = 0; i < 6; i++) {
          const a = (i * Math.PI) / 3;
          const r = flashR * (0.8 + ((i % 2) ? 0.3 : -0.1));
          ctx.lineTo(Math.cos(a) * r, Math.sin(a) * r);
        }
        ctx.closePath();
        ctx.fill();
      } else if (frac < 0.7) {
        // Expanding fiery combustion cloud
        const fireR = maxPx * (0.6 + (frac - 0.35) * 0.8);
        ctx.fillStyle = "#ea580c";
        ctx.beginPath();
        ctx.arc(0, 0, fireR, 0, Math.PI * 2);
        ctx.fill();

        ctx.fillStyle = "#f59e0b";
        ctx.beginPath();
        ctx.arc(0, -fireR * 0.2, fireR * 0.65, 0, Math.PI * 2);
        ctx.fill();

        // Dark soot core beginning to form
        ctx.fillStyle = "#27272a";
        ctx.beginPath();
        ctx.arc(0, 0, fireR * 0.4, 0, Math.PI * 2);
        ctx.fill();
      } else {
        // Billowing dark charcoal smoke dissipating
        const smokeAlpha = (1 - frac) / 0.3;
        const smokeR = maxPx * (0.9 + (frac - 0.7) * 0.4);
        ctx.fillStyle = `rgba(24, 24, 27, ${smokeAlpha * 0.85})`;
        ctx.beginPath();
        ctx.arc(0, -smokeR * 0.3, smokeR * 0.8, 0, Math.PI * 2);
        ctx.arc(-smokeR * 0.3, 0, smokeR * 0.6, 0, Math.PI * 2);
        ctx.arc(smokeR * 0.3, 0, smokeR * 0.6, 0, Math.PI * 2);
        ctx.fill();
      }

      ctx.restore();
    }

    // 3. Particles (Shrapnel sparks & rising soot)
    for (const pt of this.particles) {
      const sx = cam.screenX(pt.x);
      const sy = cam.screenY(pt.y);
      const a = (pt.life / pt.maxLife) * pt.alpha;

      ctx.fillStyle = pt.color;
      ctx.globalAlpha = Math.max(0, Math.min(1, a));
      ctx.fillRect(sx - pt.size / 2, sy - pt.size / 2, pt.size, pt.size);
      ctx.globalAlpha = 1;
    }
  }
}

export const fx = new FXEngine();
